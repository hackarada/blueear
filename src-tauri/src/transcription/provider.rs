//! The contract every transcription engine implements, plus the registry that
//! owns the concrete instances.
//!
//! Providers are deliberately narrow: given one finalized WAV file, return
//! words and optionally speaker spans, reporting progress and honouring
//! cancellation along the way. Everything else -- deciding which provider to
//! use, naming speakers, persisting results, merging tracks -- belongs to
//! `service.rs` and `merge.rs`, so that adding an engine never means
//! reimplementing job semantics.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::error::{AppError, AppResult};
use crate::transcription::types::{
    NotReadyReason, ProviderCapabilities, ProviderId, ProviderStatus, RawTrackResult, TrackRequest,
};

/// Cooperative cancellation. Providers check this between chunks; nothing is
/// ever killed mid-inference, which is why cancellation is observed at the
/// next checkpoint rather than instantly.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    /// Convenience for the common `return Err(...) if cancelled` check.
    pub fn check(&self) -> AppResult<()> {
        if self.is_cancelled() {
            Err(AppError::transcription_cancelled())
        } else {
            Ok(())
        }
    }
}

/// Everything a provider needs from the app that is not part of the request
/// itself. Kept as an explicit parameter rather than global state so tests can
/// point a provider at a temporary directory.
#[derive(Debug, Clone)]
pub struct ProviderContext {
    /// The app-owned directory holding validated model bundles. Providers load
    /// only from here, never from a path the user chose.
    pub models_root: PathBuf,
}

/// Reports a 0.0 to 1.0 fraction for the track currently being transcribed.
pub type ProgressSink<'a> = dyn Fn(f32) + Send + Sync + 'a;

pub trait TranscriptionProvider: Send + Sync {
    fn id(&self) -> ProviderId;

    fn capabilities(&self) -> ProviderCapabilities;

    /// `None` means ready. Called on every settings refresh, so it must be
    /// cheap and must not load models or allocate inference resources -- the
    /// plan requires FluidAudio stay completely inert until it is both
    /// selected and actually used.
    fn probe(&self, ctx: &ProviderContext) -> Option<NotReadyReason>;

    fn transcribe_track(
        &self,
        request: &TrackRequest,
        ctx: &ProviderContext,
        cancel: &CancelToken,
        progress: &ProgressSink<'_>,
    ) -> AppResult<RawTrackResult>;

    fn status(&self, ctx: &ProviderContext) -> ProviderStatus {
        let not_ready_reason = self.probe(ctx);
        ProviderStatus {
            capabilities: self.capabilities(),
            ready: not_ready_reason.is_none(),
            not_ready_reason,
        }
    }
}

/// The "don't transcribe" choice, modelled as a real provider so the rest of
/// the system never has to special-case an absent one. It is never ready and
/// never runs.
pub struct NoneProvider;

impl TranscriptionProvider for NoneProvider {
    fn id(&self) -> ProviderId {
        ProviderId::None
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            id: ProviderId::None,
            display_name: "None".to_string(),
            minimum_macos: "14.4".to_string(),
            minimum_windows: "10.0.20348".to_string(),
            requires_model_import: false,
            supports_remote_speaker_labels: false,
            supports_word_timings: false,
            summary: "Record audio only. No transcript is created.".to_string(),
        }
    }

    fn probe(&self, _ctx: &ProviderContext) -> Option<NotReadyReason> {
        Some(NotReadyReason::NotConfigured)
    }

    fn transcribe_track(
        &self,
        _request: &TrackRequest,
        _ctx: &ProviderContext,
        _cancel: &CancelToken,
        _progress: &ProgressSink<'_>,
    ) -> AppResult<RawTrackResult> {
        Err(AppError::transcription_provider_not_ready())
    }
}

/// Owns one instance of every provider Blue Ear knows about, in the order the
/// settings screen displays them.
pub struct ProviderRegistry {
    providers: Vec<Arc<dyn TranscriptionProvider>>,
}

impl ProviderRegistry {
    pub fn new(providers: Vec<Arc<dyn TranscriptionProvider>>) -> Self {
        Self { providers }
    }

    pub fn get(&self, id: ProviderId) -> Option<Arc<dyn TranscriptionProvider>> {
        self.providers.iter().find(|p| p.id() == id).cloned()
    }

    pub fn statuses(&self, ctx: &ProviderContext) -> Vec<ProviderStatus> {
        self.providers.iter().map(|p| p.status(ctx)).collect()
    }

    /// Whether any provider other than `none` could ever be made ready on this
    /// build and OS. When false, the settings screen says so instead of
    /// offering choices that cannot work.
    pub fn any_provider_installable(&self, ctx: &ProviderContext) -> bool {
        self.providers.iter().any(|p| {
            p.id() != ProviderId::None
                && !matches!(
                    p.probe(ctx),
                    Some(NotReadyReason::OsTooOld) | Some(NotReadyReason::NotBuilt)
                )
        })
    }
}

#[cfg(test)]
pub(crate) mod fake {
    //! A provider that returns canned results, used by the contract tests in
    //! `service.rs`. It exists so job lifecycle, cancellation, partial
    //! completion, restart recovery, and merging can all be tested without a
    //! model, an audio file, or a GPU.

    use std::sync::Mutex;

    use super::*;
    use crate::transcription::types::{NotReadyReason, Track, Word};

    pub struct FakeProvider {
        pub id: ProviderId,
        pub not_ready: Option<NotReadyReason>,
        /// Tracks listed here fail instead of succeeding, so partial-completion
        /// and track-only retry can be exercised.
        pub failing_tracks: Vec<Track>,
        /// Tracks transcribed so far, in order, across every call.
        pub calls: Mutex<Vec<Track>>,
        /// When set, the provider cancels itself partway through, simulating a
        /// user pressing Cancel mid-inference.
        pub self_cancel: bool,
    }

    impl FakeProvider {
        pub fn ready(id: ProviderId) -> Self {
            Self {
                id,
                not_ready: None,
                failing_tracks: Vec::new(),
                calls: Mutex::new(Vec::new()),
                self_cancel: false,
            }
        }

        pub fn failing_on(id: ProviderId, tracks: &[Track]) -> Self {
            Self {
                failing_tracks: tracks.to_vec(),
                ..Self::ready(id)
            }
        }

        pub fn self_cancelling(id: ProviderId) -> Self {
            Self {
                self_cancel: true,
                ..Self::ready(id)
            }
        }

        pub fn not_ready(id: ProviderId, reason: NotReadyReason) -> Self {
            Self {
                not_ready: Some(reason),
                ..Self::ready(id)
            }
        }
    }

    impl TranscriptionProvider for FakeProvider {
        fn id(&self) -> ProviderId {
            self.id
        }

        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities {
                id: self.id,
                display_name: "Fake".to_string(),
                minimum_macos: "14.4".to_string(),
                minimum_windows: String::new(),
                requires_model_import: false,
                supports_remote_speaker_labels: true,
                supports_word_timings: true,
                summary: "Test double.".to_string(),
            }
        }

        fn probe(&self, _ctx: &ProviderContext) -> Option<NotReadyReason> {
            self.not_ready
        }

        fn transcribe_track(
            &self,
            request: &TrackRequest,
            _ctx: &ProviderContext,
            cancel: &CancelToken,
            progress: &ProgressSink<'_>,
        ) -> AppResult<RawTrackResult> {
            self.calls.lock().unwrap().push(request.track);
            progress(0.5);

            if self.self_cancel {
                cancel.cancel();
            }
            cancel.check()?;

            if self.failing_tracks.contains(&request.track) {
                return Err(AppError::transcription_failed());
            }

            progress(1.0);
            let offset = match request.track {
                Track::Meeting => 0.0,
                Track::Microphone => 0.25,
            };
            Ok(RawTrackResult {
                words: vec![
                    Word {
                        text: format!("{}-one", request.track.as_str()),
                        start_seconds: offset,
                        end_seconds: offset + 0.2,
                        confidence: Some(0.9),
                    },
                    Word {
                        text: format!("{}-two", request.track.as_str()),
                        start_seconds: offset + 0.3,
                        end_seconds: offset + 0.5,
                        confidence: Some(0.9),
                    },
                ],
                speaker_spans: if request.diarize {
                    vec![crate::transcription::types::SpeakerSpan {
                        speaker_key: "cluster-0".to_string(),
                        start_seconds: 0.0,
                        end_seconds: 10.0,
                    }]
                } else {
                    Vec::new()
                },
                model_id: Some("fake-model".to_string()),
                language: Some("en".to_string()),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcription::types::Track;

    fn ctx() -> ProviderContext {
        ProviderContext {
            models_root: PathBuf::from("/tmp/blueear-test-models"),
        }
    }

    #[test]
    fn none_provider_is_never_ready_and_never_runs() {
        let provider = NoneProvider;
        let status = provider.status(&ctx());
        assert!(!status.ready);
        assert_eq!(status.not_ready_reason, Some(NotReadyReason::NotConfigured));

        let request = TrackRequest {
            track: Track::Meeting,
            audio_path: PathBuf::from("/tmp/meeting.wav"),
            diarize: false,
            language: None,
        };
        let err = provider
            .transcribe_track(&request, &ctx(), &CancelToken::new(), &|_| {})
            .unwrap_err();
        assert_eq!(
            err.code,
            crate::error::ErrorCode::TranscriptionProviderNotReady
        );
    }

    #[test]
    fn registry_resolves_providers_by_id_and_reports_every_status() {
        let registry = ProviderRegistry::new(vec![
            Arc::new(NoneProvider),
            Arc::new(fake::FakeProvider::ready(ProviderId::FluidAudio)),
        ]);

        assert!(registry.get(ProviderId::FluidAudio).is_some());
        assert!(registry.get(ProviderId::AppleSpeech).is_none());
        assert_eq!(registry.statuses(&ctx()).len(), 2);
        assert!(registry.any_provider_installable(&ctx()));
    }

    #[test]
    fn a_build_with_only_unbuildable_providers_reports_nothing_installable() {
        let registry = ProviderRegistry::new(vec![
            Arc::new(NoneProvider),
            Arc::new(fake::FakeProvider::not_ready(
                ProviderId::AppleSpeech,
                NotReadyReason::NotBuilt,
            )),
        ]);
        assert!(!registry.any_provider_installable(&ctx()));
    }

    #[test]
    fn a_cancelled_token_short_circuits_with_the_cancellation_code() {
        let token = CancelToken::new();
        assert!(token.check().is_ok());
        token.cancel();
        assert_eq!(
            token.check().unwrap_err().code,
            crate::error::ErrorCode::TranscriptionCancelled
        );
    }
}
