//! Job orchestration: turning "transcribe this finished recording" into a
//! sequence of per-track provider calls, persisted state, progress events, and
//! a merged transcript.
//!
//! This is the only place that mutates a job, mirroring how `SessionManager`
//! is the only writer of `SessionState`. Providers know nothing about jobs,
//! and the Tauri commands are thin wrappers around the methods here.

use std::sync::{Arc, Mutex};
use std::thread;

use chrono::Utc;

use crate::error::{AppError, AppResult};
use crate::storage::session_store::{self, SessionMetadata, SessionPaths};
use crate::transcription::merge;
use crate::transcription::provider::{
    CancelToken, ProviderContext, ProviderRegistry, TranscriptionProvider,
};
use crate::transcription::store::{self, ExportFormat, Preferences};
use crate::transcription::types::{
    Job, JobStatus, ProviderId, ProviderStatus, Track, TrackRequest, TrackTranscript, Transcript,
    TRANSCRIPT_SCHEMA_VERSION,
};

/// Emits job updates to the frontend. Abstracted behind a trait so the
/// contract tests can run the full lifecycle with no Tauri app, and so
/// `service.rs` has no `tauri` import at all.
pub trait JobObserver: Send + Sync {
    fn job_changed(&self, job: &Job);
}

/// Discards updates. Used by tests that only assert on the persisted record.
#[cfg(test)]
pub struct NullObserver;

#[cfg(test)]
impl JobObserver for NullObserver {
    fn job_changed(&self, _job: &Job) {}
}

/// The whole transcription settings surface as one snapshot.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptionOverview {
    pub providers: Vec<ProviderStatus>,
    /// False when no engine could ever be readied on this build and OS, which
    /// lets the UI explain that instead of offering unreachable choices.
    pub any_provider_installable: bool,
    pub preferences: Preferences,
    pub installed_bundles: Vec<crate::transcription::model_import::InstalledBundle>,
}

/// The currently running job, if any. Blue Ear runs one transcription at a
/// time: these are multi-minute CoreML workloads, and two of them competing
/// for the Neural Engine would make both slower and the progress meaningless.
struct RunningJob {
    session_id: String,
    cancel: CancelToken,
}

pub struct TranscriptionService {
    registry: ProviderRegistry,
    observer: Arc<dyn JobObserver>,
    running: Mutex<Option<RunningJob>>,
}

impl TranscriptionService {
    pub fn new(registry: ProviderRegistry, observer: Arc<dyn JobObserver>) -> Arc<Self> {
        Arc::new(Self {
            registry,
            observer,
            running: Mutex::new(None),
        })
    }

    fn context(&self) -> ProviderContext {
        ProviderContext {
            models_root: store::models_root(),
        }
    }

    // MARK: - Capabilities and preferences

    /// Everything the settings screen needs, in one call. Assembled together
    /// so the UI can never render a provider's readiness against a stale
    /// selection or a bundle list from a moment earlier.
    pub fn overview(&self) -> TranscriptionOverview {
        let context = self.context();
        TranscriptionOverview {
            providers: self.registry.statuses(&context),
            any_provider_installable: self.registry.any_provider_installable(&context),
            preferences: self.preferences(),
            installed_bundles: crate::transcription::model_import::list_installed_bundles(),
        }
    }

    pub fn preferences(&self) -> Preferences {
        store::read_preferences()
    }

    pub fn set_preferences(&self, preferences: &Preferences) -> AppResult<()> {
        store::write_preferences(preferences).map_err(|_| AppError::internal("write_preferences"))
    }

    /// The provider the user selected, but only if it is actually ready.
    /// Returns an actionable error rather than substituting another engine:
    /// silently transcribing with something the user did not choose is exactly
    /// the behaviour the design forbids.
    fn ready_provider(&self, id: ProviderId) -> AppResult<Arc<dyn TranscriptionProvider>> {
        if id == ProviderId::None {
            return Err(AppError::transcription_provider_not_ready());
        }
        let provider = self
            .registry
            .get(id)
            .ok_or_else(AppError::transcription_unavailable)?;
        if provider.probe(&self.context()).is_some() {
            return Err(AppError::transcription_provider_not_ready());
        }
        Ok(provider)
    }

    // MARK: - Reading persisted state

    fn session_paths(&self, session_id: &str) -> AppResult<SessionPaths> {
        session_store::find_session_by_id(session_id)
            .map_err(|_| AppError::internal("find_session_by_id"))?
            .ok_or_else(AppError::session_not_found)
    }

    pub fn job(&self, session_id: &str) -> AppResult<Option<Job>> {
        let paths = self.session_paths(session_id)?;
        store::read_job(&paths).map_err(|_| AppError::internal("read_job"))
    }

    pub fn transcript(&self, session_id: &str) -> AppResult<Transcript> {
        let paths = self.session_paths(session_id)?;
        store::read_transcript(&paths)
            .map_err(|_| AppError::internal("read_transcript"))?
            .ok_or_else(AppError::transcript_not_found)
    }

    pub fn export(&self, session_id: &str, format: &str) -> AppResult<String> {
        let format = ExportFormat::from_str(format).ok_or_else(AppError::track_not_found)?;
        let paths = self.session_paths(session_id)?;
        let transcript = store::read_transcript(&paths)
            .map_err(|_| AppError::internal("read_transcript"))?
            .ok_or_else(AppError::transcript_not_found)?;
        let path = store::write_export(&paths, &transcript, format)
            .map_err(|_| AppError::internal("write_export"))?;
        Ok(path.to_string_lossy().into_owned())
    }

    // MARK: - Job lifecycle

    /// Queues a transcription for a finalized session and returns immediately;
    /// the work happens on a background thread.
    pub fn start(self: &Arc<Self>, session_id: &str) -> AppResult<Job> {
        self.start_inner(session_id, false)
    }

    /// Reruns only the tracks that are not already complete, preserving any
    /// track that succeeded on an earlier attempt.
    pub fn retry(self: &Arc<Self>, session_id: &str) -> AppResult<Job> {
        self.start_inner(session_id, true)
    }

    /// Runs a job only if the user asked for automatic transcription and a
    /// provider is actually ready. Called from the finalized-session hook, so
    /// it must stay silent about every reason it might decline.
    pub fn maybe_start_automatically(self: &Arc<Self>, session_id: &str) {
        let preferences = self.preferences();
        if !preferences.auto_transcribe {
            return;
        }
        if self.ready_provider(preferences.provider).is_err() {
            return;
        }
        let _ = self.start(session_id);
    }

    fn start_inner(self: &Arc<Self>, session_id: &str, is_retry: bool) -> AppResult<Job> {
        let paths = self.session_paths(session_id)?;
        let metadata =
            session_store::read_metadata(&paths).map_err(|_| AppError::internal("read_metadata"))?;

        let preferences = self.preferences();
        let provider = self.ready_provider(preferences.provider)?;

        let diarize = preferences.diarize_remote_speakers
            && provider.capabilities().supports_remote_speaker_labels;

        let mut job = self.prepare_job(&paths, &metadata, preferences.provider, diarize, is_retry)?;

        {
            let mut running = self.running.lock().unwrap();
            if running.is_some() {
                return Err(AppError::session_conflict());
            }
            *running = Some(RunningJob {
                session_id: session_id.to_string(),
                cancel: CancelToken::new(),
            });
        }

        job.status = JobStatus::Preparing;
        job.error = None;
        self.commit(&paths, &mut job);

        let service = Arc::clone(self);
        let worker_paths = SessionPaths {
            dir: paths.dir.clone(),
        };
        let worker_job = job.clone();
        let language = preferences.language.clone();
        thread::spawn(move || {
            service.run_job(worker_paths, metadata, provider, worker_job, language);
        });

        Ok(job)
    }

    /// Builds the job record this attempt will use: a fresh one for a first
    /// run, or the previous one with its attempt counter bumped and its
    /// completed tracks left alone for a retry.
    fn prepare_job(
        &self,
        paths: &SessionPaths,
        metadata: &SessionMetadata,
        provider: ProviderId,
        diarize: bool,
        is_retry: bool,
    ) -> AppResult<Job> {
        let tracks = available_tracks(metadata);
        if tracks.is_empty() {
            return Err(AppError::track_not_found());
        }

        let existing = store::read_job(paths).map_err(|_| AppError::internal("read_job"))?;

        let mut job = match (is_retry, existing) {
            (true, Some(mut previous)) => {
                previous.attempt += 1;
                previous.provider = provider;
                previous.diarize = diarize;
                previous.started_at = Utc::now();
                for track in &mut previous.tracks {
                    if track.status != JobStatus::Completed {
                        track.status = JobStatus::Queued;
                        track.progress = 0.0;
                        track.error = None;
                    }
                }
                previous
            }
            _ => Job::new(&metadata.session_id, provider, &tracks, diarize),
        };

        if job.pending_tracks().is_empty() {
            return Err(AppError::session_conflict());
        }
        job.refresh();
        Ok(job)
    }

    fn run_job(
        self: Arc<Self>,
        paths: SessionPaths,
        metadata: SessionMetadata,
        provider: Arc<dyn TranscriptionProvider>,
        mut job: Job,
        language: Option<String>,
    ) {
        let cancel = self
            .running
            .lock()
            .unwrap()
            .as_ref()
            .map(|r| r.cancel.clone())
            .unwrap_or_default();

        job.status = JobStatus::Transcribing;
        self.commit(&paths, &mut job);

        // Track results from earlier attempts are reused verbatim, which is
        // what makes a retry cost only the track that actually failed.
        let mut track_results: Vec<TrackTranscript> = store::read_transcript(&paths)
            .ok()
            .flatten()
            .map(|t| t.tracks)
            .unwrap_or_default();

        let context = self.context();
        let mut cancelled = false;

        for track in job.pending_tracks() {
            if cancel.is_cancelled() {
                cancelled = true;
                break;
            }

            if let Some(entry) = job.track_mut(track) {
                entry.status = JobStatus::Transcribing;
                entry.error = None;
            }
            self.commit(&paths, &mut job);

            let request = TrackRequest {
                track,
                audio_path: match track {
                    Track::Meeting => paths.meeting_wav(),
                    Track::Microphone => paths.microphone_wav(),
                },
                // Only the meeting track carries unknown speakers; the
                // microphone track is the local user by construction.
                diarize: job.diarize && track == Track::Meeting,
                language: language.clone(),
            };

            let progress_paths = SessionPaths {
                dir: paths.dir.clone(),
            };
            let progress_service = Arc::clone(&self);
            let progress_job = Mutex::new(job.clone());
            let sink = move |fraction: f32| {
                let mut guard = progress_job.lock().unwrap();
                if let Some(entry) = guard.track_mut(track) {
                    entry.progress = fraction.clamp(0.0, 1.0);
                }
                let mut snapshot = guard.clone();
                drop(guard);
                progress_service.commit(&progress_paths, &mut snapshot);
            };

            match provider.transcribe_track(&request, &context, &cancel, &sink) {
                Ok(raw) => {
                    track_results.retain(|t| t.track != track);
                    track_results.push(merge::build_track_transcript(track, provider.id(), &raw));
                    if let Some(entry) = job.track_mut(track) {
                        entry.status = JobStatus::Completed;
                        entry.progress = 1.0;
                    }
                }
                Err(error) => {
                    let is_cancellation =
                        error.code == crate::error::ErrorCode::TranscriptionCancelled;
                    if let Some(entry) = job.track_mut(track) {
                        entry.status = if is_cancellation {
                            JobStatus::Cancelled
                        } else {
                            JobStatus::Failed
                        };
                        entry.error = Some(error);
                    }
                    if is_cancellation {
                        cancelled = true;
                        self.commit(&paths, &mut job);
                        break;
                    }
                }
            }
            self.commit(&paths, &mut job);
        }

        if !cancelled {
            job.status = JobStatus::Merging;
            self.commit(&paths, &mut job);
        }

        // A partial result is still worth keeping: one good track beats no
        // transcript, and it is what lets a retry redo only the other one.
        if !track_results.is_empty() {
            track_results.sort_by_key(|t| t.track.order());
            let transcript = Transcript {
                schema_version: TRANSCRIPT_SCHEMA_VERSION,
                session_id: metadata.session_id.clone(),
                provider: provider.id(),
                created_at: Utc::now(),
                duration_seconds: metadata.duration_seconds,
                segments: merge::merge_tracks(&track_results),
                tracks: track_results,
            };
            if store::write_transcript(&paths, &transcript).is_err() {
                log::error!("failed to persist transcript for a finalized session");
            }
        }

        job.status = if cancelled {
            JobStatus::Cancelled
        } else if job.tracks.iter().any(|t| t.status == JobStatus::Failed) {
            job.error = Some(AppError::transcription_failed());
            JobStatus::Failed
        } else {
            JobStatus::Completed
        };
        self.commit(&paths, &mut job);

        *self.running.lock().unwrap() = None;
    }

    pub fn cancel(&self, session_id: &str) -> AppResult<()> {
        let running = self.running.lock().unwrap();
        match running.as_ref() {
            Some(job) if job.session_id == session_id => {
                job.cancel.cancel();
                Ok(())
            }
            _ => Err(AppError::session_not_found()),
        }
    }

    /// Persists the job and notifies the frontend in one step, so an observer
    /// can never see a state that was not also written to disk.
    fn commit(&self, paths: &SessionPaths, job: &mut Job) {
        job.refresh();
        if store::write_job(paths, job).is_err() {
            log::error!("failed to persist a transcription job record");
        }
        self.observer.job_changed(job);
    }
}

/// Which tracks a finalized session actually has. `mixed.wav` is excluded by
/// design; see `types::Track`.
fn available_tracks(metadata: &SessionMetadata) -> Vec<Track> {
    let mut tracks = Vec::new();
    if metadata.meeting_wav.is_some() {
        tracks.push(Track::Meeting);
    }
    if metadata.microphone_wav.is_some() {
        tracks.push(Track::Microphone);
    }
    tracks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use crate::storage::session_store::SCHEMA_VERSION;
    use crate::test_support::HOME_ENV_LOCK;
    use crate::transcription::provider::{fake::FakeProvider, NoneProvider};
    use crate::transcription::types::{NotReadyReason, Speaker};
    use chrono::Local;
    use std::sync::MutexGuard;
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    struct Harness {
        _home: TempDir,
        _guard: MutexGuard<'static, ()>,
        session_id: String,
    }

    /// Creates a finalized session on disk under a temporary `HOME`, so the
    /// whole service can be driven without a Tauri app or a real recording.
    fn harness(mic_enabled: bool) -> Harness {
        let guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let home = TempDir::new().unwrap();
        std::env::set_var("HOME", home.path());

        let session_id = uuid::Uuid::new_v4().to_string();
        let paths = session_store::create_in_progress_dir(&session_id, crate::audio::MeetingAppId::Teams).unwrap();
        std::fs::write(paths.meeting_wav(), b"fake wav").unwrap();
        if mic_enabled {
            std::fs::write(paths.microphone_wav(), b"fake wav").unwrap();
        }
        let metadata = SessionMetadata {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.clone(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            duration_seconds: 10.0,
            mic_enabled,
            recovered: false,
            interrupted: false,
            dropped_meeting_frames: 0,
            dropped_mic_frames: 0,
            sample_rate: 48_000,
            meeting_wav: Some("meeting.wav".into()),
            microphone_wav: mic_enabled.then(|| "microphone.wav".into()),
            mixed_wav: "mixed.wav".into(),
            source_app: crate::audio::MeetingAppId::Teams,
            app_bundle_id: crate::APP_BUNDLE_ID.into(),
            app_version: "0.1.0".into(),
        };
        session_store::write_metadata(&paths, &metadata).unwrap();
        session_store::finalize_dir(&paths, Local::now()).unwrap();

        Harness {
            _home: home,
            _guard: guard,
            session_id,
        }
    }

    fn service_with(provider: Arc<dyn TranscriptionProvider>) -> Arc<TranscriptionService> {
        TranscriptionService::new(
            ProviderRegistry::new(vec![Arc::new(NoneProvider), provider]),
            Arc::new(NullObserver),
        )
    }

    fn select(provider: ProviderId, auto: bool) {
        store::write_preferences(&Preferences {
            provider,
            auto_transcribe: auto,
            ..Preferences::default()
        })
        .unwrap();
    }

    /// Jobs run on a background thread; poll the persisted record rather than
    /// sleeping for a fixed duration.
    fn wait_for_terminal(service: &TranscriptionService, session_id: &str) -> Job {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(Some(job)) = service.job(session_id) {
                if job.status.is_terminal() {
                    return job;
                }
            }
            assert!(Instant::now() < deadline, "job never reached a terminal state");
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_session_is_not_transcribed_when_no_provider_is_configured() {
        let h = harness(true);
        let service = service_with(Arc::new(FakeProvider::ready(ProviderId::FluidAudio)));
        select(ProviderId::None, false);

        let err = service.start(&h.session_id).unwrap_err();
        assert_eq!(err.code, ErrorCode::TranscriptionProviderNotReady);
        assert!(service.job(&h.session_id).unwrap().is_none());
    }

    #[test]
    fn a_selected_but_unready_provider_fails_instead_of_falling_back() {
        let h = harness(true);
        let service = TranscriptionService::new(
            ProviderRegistry::new(vec![
                Arc::new(NoneProvider),
                Arc::new(FakeProvider::not_ready(
                    ProviderId::FluidAudio,
                    NotReadyReason::ModelsMissing,
                )),
                Arc::new(FakeProvider::ready(ProviderId::AppleSpeech)),
            ]),
            Arc::new(NullObserver),
        );
        select(ProviderId::FluidAudio, false);

        let err = service.start(&h.session_id).unwrap_err();
        assert_eq!(err.code, ErrorCode::TranscriptionProviderNotReady);
        assert!(
            service.transcript(&h.session_id).is_err(),
            "the ready provider must not have been substituted in"
        );
    }

    #[test]
    fn both_tracks_are_transcribed_and_merged_into_one_timeline() {
        let h = harness(true);
        let service = service_with(Arc::new(FakeProvider::ready(ProviderId::FluidAudio)));
        select(ProviderId::FluidAudio, false);

        service.start(&h.session_id).unwrap();
        let job = wait_for_terminal(&service, &h.session_id);

        assert_eq!(job.status, JobStatus::Completed);
        assert!((job.progress - 1.0).abs() < 1e-6);

        let transcript = service.transcript(&h.session_id).unwrap();
        assert_eq!(transcript.tracks.len(), 2);
        assert!(transcript
            .segments
            .windows(2)
            .all(|w| w[0].start_seconds <= w[1].start_seconds));
        assert!(transcript
            .segments
            .iter()
            .any(|s| s.speaker == Speaker::You));
    }

    #[test]
    fn a_failing_track_preserves_the_successful_one_and_retry_redoes_only_the_failure() {
        let h = harness(true);
        let provider = Arc::new(FakeProvider::failing_on(
            ProviderId::FluidAudio,
            &[Track::Microphone],
        ));
        let service = service_with(provider.clone());
        select(ProviderId::FluidAudio, false);

        service.start(&h.session_id).unwrap();
        let job = wait_for_terminal(&service, &h.session_id);

        assert_eq!(job.status, JobStatus::Failed);
        let transcript = service.transcript(&h.session_id).unwrap();
        assert_eq!(transcript.tracks.len(), 1);
        assert_eq!(transcript.tracks[0].track, Track::Meeting);

        provider.calls.lock().unwrap().clear();
        service.retry(&h.session_id).unwrap();
        let retried = wait_for_terminal(&service, &h.session_id);

        assert_eq!(retried.attempt, 2);
        assert_eq!(
            *provider.calls.lock().unwrap(),
            vec![Track::Microphone],
            "retry must not redo the track that already succeeded"
        );
    }

    #[test]
    fn cancellation_ends_the_job_without_marking_it_failed() {
        let h = harness(true);
        let service = service_with(Arc::new(FakeProvider::self_cancelling(
            ProviderId::FluidAudio,
        )));
        select(ProviderId::FluidAudio, false);

        service.start(&h.session_id).unwrap();
        let job = wait_for_terminal(&service, &h.session_id);

        assert_eq!(job.status, JobStatus::Cancelled);
        assert!(job.error.is_none());
    }

    #[test]
    fn a_session_without_a_microphone_track_transcribes_the_meeting_track_alone() {
        let h = harness(false);
        let provider = Arc::new(FakeProvider::ready(ProviderId::FluidAudio));
        let service = service_with(provider.clone());
        select(ProviderId::FluidAudio, false);

        service.start(&h.session_id).unwrap();
        wait_for_terminal(&service, &h.session_id);

        assert_eq!(*provider.calls.lock().unwrap(), vec![Track::Meeting]);
    }

    #[test]
    fn an_interrupted_job_is_recovered_as_retryable_on_the_next_launch() {
        let h = harness(true);
        let service = service_with(Arc::new(FakeProvider::ready(ProviderId::FluidAudio)));
        select(ProviderId::FluidAudio, false);

        // Simulate a force-quit mid-transcription: a job file left in a
        // non-terminal state with nothing running.
        let paths = session_store::find_session_by_id(&h.session_id)
            .unwrap()
            .unwrap();
        let mut job = Job::new(
            &h.session_id,
            ProviderId::FluidAudio,
            &[Track::Meeting, Track::Microphone],
            true,
        );
        job.status = JobStatus::Transcribing;
        job.track_mut(Track::Meeting).unwrap().status = JobStatus::Completed;
        store::write_job(&paths, &job).unwrap();

        assert_eq!(store::recover_interrupted_jobs(), 1);

        let recovered = service.job(&h.session_id).unwrap().unwrap();
        assert_eq!(recovered.status, JobStatus::Failed);
        assert_eq!(
            recovered.error.as_ref().unwrap().code,
            ErrorCode::TranscriptionInterrupted
        );
        assert_eq!(
            recovered.pending_tracks(),
            vec![Track::Microphone],
            "the track that finished before the crash must survive"
        );
    }

    #[test]
    fn auto_transcribe_is_a_no_op_until_it_is_both_enabled_and_ready() {
        let h = harness(true);
        let provider = Arc::new(FakeProvider::ready(ProviderId::FluidAudio));
        let service = service_with(provider.clone());

        // Off by default.
        select(ProviderId::FluidAudio, false);
        service.maybe_start_automatically(&h.session_id);
        assert!(service.job(&h.session_id).unwrap().is_none());

        // On, but with no provider configured.
        select(ProviderId::None, true);
        service.maybe_start_automatically(&h.session_id);
        assert!(service.job(&h.session_id).unwrap().is_none());

        // On, with a ready provider.
        select(ProviderId::FluidAudio, true);
        service.maybe_start_automatically(&h.session_id);
        wait_for_terminal(&service, &h.session_id);
        assert!(!provider.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn exports_are_derived_from_the_persisted_transcript() {
        let h = harness(true);
        let service = service_with(Arc::new(FakeProvider::ready(ProviderId::FluidAudio)));
        select(ProviderId::FluidAudio, false);

        service.start(&h.session_id).unwrap();
        wait_for_terminal(&service, &h.session_id);

        let text_path = service.export(&h.session_id, "text").unwrap();
        assert!(std::fs::read_to_string(&text_path).unwrap().contains("You:"));
        let vtt_path = service.export(&h.session_id, "vtt").unwrap();
        assert!(std::fs::read_to_string(&vtt_path).unwrap().starts_with("WEBVTT"));
    }

    #[test]
    fn exporting_before_transcribing_reports_a_missing_transcript() {
        let h = harness(true);
        let service = service_with(Arc::new(FakeProvider::ready(ProviderId::FluidAudio)));
        assert_eq!(
            service.export(&h.session_id, "text").unwrap_err().code,
            ErrorCode::TranscriptNotFound
        );
    }
}
