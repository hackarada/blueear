//! Orchestrates the whole recording lifecycle: readiness checks, permission
//! probing, native capture start/stop, the streaming alignment/mixing
//! worker, crash-safe finalization, and the Tauri events the frontend
//! listens for (`session-state`, `levels`, `source-warning`).
//!
//! This is the single place that mutates [`SessionState`]; every Tauri
//! command in `commands.rs` is a thin wrapper around a method here.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use uuid::Uuid;

use crate::audio::aligner::{silence_frames_needed, target_frame_index};
use crate::audio::mixer::mix_frame;
use crate::audio::resampler::{downmix_to_mono, resample_linear, CANONICAL_SAMPLE_RATE};
use crate::audio::ring::{self, AudioChunk, RingConsumer};
use crate::audio::{MeetingAppId, PlatformAudioEngine, StatusEvent};
use crate::error::{AppError, AppResult};
use crate::paths;
use crate::storage::session_store::{self, SessionMetadata, SessionPaths};
use crate::storage::wav_writer::CheckpointingWavWriter;

use super::state::SessionState;

const LOW_DISK_THRESHOLD_BYTES: u64 = 200 * 1024 * 1024; // 200 MB

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PermissionState {
    Unknown,
    Granted,
    Denied,
    NeedsProbe,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MeetingAppStatus {
    pub id: MeetingAppId,
    pub display_name: String,
    pub installed: bool,
    pub running: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Readiness {
    pub os_supported: bool,
    /// `macos` | `windows` | `other` — drives permission and path copy in the UI.
    pub platform: String,
    /// Human-readable recordings directory for onboarding (not a filesystem probe).
    pub recordings_path_display: String,
    pub meeting_apps: Vec<MeetingAppStatus>,
    pub microphone_available: bool,
    pub permission_state: PermissionState,
}

/// Lock-free-ish (single relaxed atomic per source) peak level tracker read
/// by a 10Hz emitter thread and written by the recording worker. Values are
/// "peak since last read", not a true RMS meter, which is enough for a
/// simple UI level bar without adding synchronization to the hot path.
struct LevelMeters {
    meeting_bits: AtomicU32,
    mic_bits: AtomicU32,
}

impl LevelMeters {
    fn new() -> Self {
        Self {
            meeting_bits: AtomicU32::new(0),
            mic_bits: AtomicU32::new(0),
        }
    }
    fn set_meeting(&self, v: f32) {
        self.meeting_bits.store(v.to_bits(), Ordering::Relaxed);
    }
    fn set_mic(&self, v: f32) {
        self.mic_bits.store(v.to_bits(), Ordering::Relaxed);
    }
    fn snapshot(&self) -> (f32, f32) {
        (
            f32::from_bits(self.meeting_bits.swap(0, Ordering::Relaxed)),
            f32::from_bits(self.mic_bits.swap(0, Ordering::Relaxed)),
        )
    }
}

struct WorkerOutcome {
    meeting_frames: u64,
    mic_frames: u64,
    dropped_meeting_frames: u64,
    dropped_mic_frames: u64,
    disk_full: bool,
    write_failed: bool,
}

struct ActiveSession {
    session_id: String,
    started_at: DateTime<Utc>,
    mic_enabled: bool,
    source_app: MeetingAppId,
    stop_flag: Arc<AtomicBool>,
    worker: JoinHandle<WorkerOutcome>,
    paths: SessionPaths,
}

pub struct SessionManager {
    engine: PlatformAudioEngine,
    app_handle: AppHandle,
    state: Mutex<SessionState>,
    permission_state: Mutex<PermissionState>,
    /// Last mic-on/off choice from the main window; the menu bar reuses it.
    last_include_microphone: Mutex<bool>,
    /// Last meeting-app choice; the menu bar reuses it.
    last_source_app: Mutex<MeetingAppId>,
    active: Mutex<Option<ActiveSession>>,
}

impl SessionManager {
    pub fn new(engine: PlatformAudioEngine, app_handle: AppHandle) -> Arc<Self> {
        Arc::new(Self {
            engine,
            app_handle,
            state: Mutex::new(SessionState::Idle),
            permission_state: Mutex::new(PermissionState::NeedsProbe),
            last_include_microphone: Mutex::new(false),
            last_source_app: Mutex::new(MeetingAppId::Teams),
            active: Mutex::new(None),
        })
    }

    pub fn spawn_status_listener(
        self: &Arc<Self>,
        status_rx: std::sync::mpsc::Receiver<(StatusEvent, i32)>,
    ) {
        let manager = Arc::clone(self);
        thread::spawn(move || {
            for (event, detail) in status_rx {
                manager.handle_status_event(event, detail);
            }
        });
    }

    fn handle_status_event(&self, event: StatusEvent, detail: i32) {
        match event {
            StatusEvent::SourceProcessTreeChanged => {
                self.transition(|state| match state {
                    SessionState::Recording {
                        session_id,
                        started_at_ms,
                        mic_enabled,
                        source_app,
                    } => Some(SessionState::Recovering {
                        session_id: session_id.clone(),
                        started_at_ms: *started_at_ms,
                        mic_enabled: *mic_enabled,
                        source_app: *source_app,
                    }),
                    _ => None,
                });
                self.emit_source_warning("source_process_tree_changed");
            }
            StatusEvent::SourceTapStarted => {
                self.transition(|state| match state {
                    SessionState::Recovering {
                        session_id,
                        started_at_ms,
                        mic_enabled,
                        source_app,
                    } => Some(SessionState::Recording {
                        session_id: session_id.clone(),
                        started_at_ms: *started_at_ms,
                        mic_enabled: *mic_enabled,
                        source_app: *source_app,
                    }),
                    _ => None,
                });
            }
            StatusEvent::SourceSilentWarning => self.emit_source_warning("source_silent"),
            StatusEvent::SourceRestored => self.emit_source_warning("source_restored"),
            StatusEvent::SourceAppNotFound => {
                // Swift passes the MeetingAppId discriminant as `detail`.
                if let Some(app) = MeetingAppId::from_i32(detail) {
                    log::warn!("{} process tree disappeared during capture", app.display_name());
                }
                self.emit_source_warning("source_app_not_found");
            }
            StatusEvent::MicDeviceChanged => self.emit_source_warning("microphone_device_changed"),
            StatusEvent::AudioPermissionDenied => {
                *self.permission_state.lock().unwrap() = PermissionState::Denied;
            }
            StatusEvent::AudioPermissionGranted => {
                *self.permission_state.lock().unwrap() = PermissionState::Granted;
            }
            StatusEvent::GenericError => self.emit_source_warning("native_error"),
            StatusEvent::SourceTapStopped | StatusEvent::MicStarted | StatusEvent::MicStopped => {}
        }
    }

    fn transition(&self, f: impl FnOnce(&SessionState) -> Option<SessionState>) {
        let mut guard = self.state.lock().unwrap();
        if let Some(next) = f(&guard) {
            *guard = next.clone();
            drop(guard);
            self.emit_state(&next);
        }
    }

    fn set_state(&self, next: SessionState) {
        *self.state.lock().unwrap() = next.clone();
        self.emit_state(&next);
    }

    fn emit_state(&self, state: &SessionState) {
        let _ = self.app_handle.emit("session-state", state);
        self.sync_tray_label(state);
    }

    /// Keeps the menu bar toggle item's label in sync with the same state
    /// transitions the frontend's `session-state` event reflects. Uses
    /// `try_state` since the tray may not have finished building yet during
    /// very early setup, or may be absent entirely (e.g. in unit tests that
    /// construct a `SessionManager` without a running Tauri app).
    fn sync_tray_label(&self, state: &SessionState) {
        let Some(tray) = self.app_handle.try_state::<crate::TrayHandles>() else {
            return;
        };
        let (label, enabled) = match state {
            SessionState::Idle | SessionState::Completed { .. } | SessionState::Failed { .. } => {
                ("Start Recording", true)
            }
            SessionState::Recording { .. } | SessionState::Recovering { .. } => {
                ("Stop Recording", true)
            }
            // Preparing / Stopping / Finalizing.
            _ => ("Working...", false),
        };
        let _ = tray.toggle_item.set_text(label);
        let _ = tray.toggle_item.set_enabled(enabled);
    }

    fn emit_source_warning(&self, code: &str) {
        let _ = self
            .app_handle
            .emit("source-warning", serde_json::json!({ "code": code }));
    }

    // MARK: - Readiness

    pub fn get_readiness(&self) -> Readiness {
        let os_supported = self.engine.os_supported();
        let meeting_apps = MeetingAppId::ALL
            .iter()
            .copied()
            .map(|id| MeetingAppStatus {
                id,
                display_name: id.display_name().to_string(),
                installed: self.engine.is_meeting_app_installed(id),
                running: self.engine.is_meeting_app_running(id),
            })
            .collect();
        Readiness {
            os_supported,
            platform: current_platform().to_string(),
            recordings_path_display: paths::recordings_path_display(),
            meeting_apps,
            microphone_available: self.engine.microphone_input_available(),
            permission_state: *self.permission_state.lock().unwrap(),
        }
    }

    pub fn request_capture_access(&self) -> PermissionState {
        let granted = self.engine.probe_audio_permission();
        let new_state = if granted {
            PermissionState::Granted
        } else {
            PermissionState::Denied
        };
        *self.permission_state.lock().unwrap() = new_state;
        new_state
    }

    pub fn preferred_include_microphone(&self) -> bool {
        *self.last_include_microphone.lock().unwrap()
    }

    pub fn preferred_source_app(&self) -> MeetingAppId {
        *self.last_source_app.lock().unwrap()
    }

    pub fn get_state(&self) -> SessionState {
        self.state.lock().unwrap().clone()
    }

    // MARK: - Recording lifecycle

    pub fn start_recording(
        &self,
        source_app: MeetingAppId,
        include_microphone: bool,
    ) -> AppResult<String> {
        let session_id = Uuid::new_v4().to_string();
        {
            let mut guard = self.state.lock().unwrap();
            if guard.is_busy() {
                return Err(AppError::session_conflict());
            }
            *guard = SessionState::Preparing {
                session_id: session_id.clone(),
            };
        }
        self.emit_state(&self.get_state());

        if let Err(err) = self.start_recording_inner(&session_id, source_app, include_microphone) {
            self.set_state(SessionState::Failed { error: err.clone() });
            return Err(err);
        }

        Ok(session_id)
    }

    fn start_recording_inner(
        &self,
        session_id: &str,
        source_app: MeetingAppId,
        include_microphone: bool,
    ) -> AppResult<()> {
        if !self.engine.os_supported() {
            return Err(AppError::unsupported_os());
        }
        if *self.permission_state.lock().unwrap() != PermissionState::Granted {
            return Err(AppError::audio_permission_denied());
        }
        if !self.engine.is_meeting_app_running(source_app) {
            return Err(if self.engine.is_meeting_app_installed(source_app) {
                AppError::meeting_app_not_running(source_app.display_name())
            } else {
                AppError::meeting_app_not_found(source_app.display_name())
            });
        }

        let paths = session_store::create_in_progress_dir(session_id, source_app)
            .map_err(|_| AppError::internal("create_in_progress_dir"))?;

        if let Some(free) = session_store::available_bytes(&paths.dir) {
            if free < LOW_DISK_THRESHOLD_BYTES {
                let _ = std::fs::remove_dir_all(&paths.dir);
                return Err(AppError::disk_full());
            }
        }

        if include_microphone && !self.engine.microphone_input_available() {
            return Err(AppError::mic_unavailable());
        }

        let sample_rate = CANONICAL_SAMPLE_RATE;
        let meeting_writer = CheckpointingWavWriter::create(&paths.meeting_wav(), sample_rate)
            .map_err(|_| AppError::internal("create_meeting_wav"))?;
        let mic_writer = if include_microphone {
            Some(
                CheckpointingWavWriter::create(&paths.microphone_wav(), sample_rate)
                    .map_err(|_| AppError::internal("create_microphone_wav"))?,
            )
        } else {
            None
        };
        let mixed_writer = CheckpointingWavWriter::create(&paths.mixed_wav(), sample_rate)
            .map_err(|_| AppError::internal("create_mixed_wav"))?;

        let (meeting_producer, meeting_consumer) = ring::channel();
        if let Err(code) = self.engine.start_meeting_capture(source_app, meeting_producer) {
            let _ = std::fs::remove_dir_all(&paths.dir);
            return Err(map_meeting_start_error(code, source_app));
        }

        let mic_consumer: Option<RingConsumer> = if include_microphone {
            let (mic_producer, mic_consumer) = ring::channel();
            if let Err(code) = self.engine.start_microphone_capture(mic_producer) {
                self.engine.stop_meeting_capture();
                let _ = std::fs::remove_dir_all(&paths.dir);
                return Err(map_mic_start_error(code));
            }
            Some(mic_consumer)
        } else {
            None
        };

        *self.last_include_microphone.lock().unwrap() = include_microphone;
        *self.last_source_app.lock().unwrap() = source_app;

        let stop_flag = Arc::new(AtomicBool::new(false));
        let disk_low_flag = Arc::new(AtomicBool::new(false));
        let io_failed_flag = Arc::new(AtomicBool::new(false));
        let levels = Arc::new(LevelMeters::new());

        let worker_paths = paths.clone_paths();
        let worker_stop_flag = Arc::clone(&stop_flag);
        let worker_disk_low = Arc::clone(&disk_low_flag);
        let worker_io_failed = Arc::clone(&io_failed_flag);
        let worker_levels = Arc::clone(&levels);
        let worker = thread::spawn(move || {
            recording_worker(
                meeting_consumer,
                mic_consumer,
                worker_paths,
                include_microphone,
                meeting_writer,
                mic_writer,
                mixed_writer,
                worker_stop_flag,
                worker_disk_low,
                worker_io_failed,
                worker_levels,
            )
        });

        let started_at = Utc::now();
        *self.active.lock().unwrap() = Some(ActiveSession {
            session_id: session_id.to_string(),
            started_at,
            mic_enabled: include_microphone,
            source_app,
            stop_flag: Arc::clone(&stop_flag),
            worker,
            paths,
        });

        self.set_state(SessionState::Recording {
            session_id: session_id.to_string(),
            started_at_ms: started_at.timestamp_millis(),
            mic_enabled: include_microphone,
            source_app,
        });

        // 10Hz level meter emitter for the lifetime of this recording.
        let app_handle = self.app_handle.clone();
        let emitter_stop_flag = Arc::clone(&stop_flag);
        let emitter_levels = Arc::clone(&levels);
        thread::spawn(move || {
            while !emitter_stop_flag.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(100));
                let (meeting, microphone) = emitter_levels.snapshot();
                let _ = app_handle.emit(
                    "levels",
                    serde_json::json!({ "meeting": meeting, "microphone": microphone }),
                );
            }
        });

        // When disk space runs low, warn the user and stop the session so
        // native capture does not keep filling a ring nobody is draining.
        let monitor_app = self.app_handle.clone();
        let monitor_stop = Arc::clone(&stop_flag);
        let monitor_disk = Arc::clone(&disk_low_flag);
        let monitor_io = Arc::clone(&io_failed_flag);
        let monitor_session = session_id.to_string();
        thread::spawn(move || {
            while !monitor_stop.load(Ordering::Relaxed) {
                if monitor_disk.load(Ordering::Relaxed) {
                    let _ = monitor_app.emit(
                        "source-warning",
                        serde_json::json!({ "code": "disk_space_low" }),
                    );
                    if let Some(manager) = monitor_app.try_state::<Arc<SessionManager>>() {
                        let _ = manager.stop_recording(&monitor_session);
                    }
                    return;
                }
                if monitor_io.load(Ordering::Relaxed) {
                    if let Some(manager) = monitor_app.try_state::<Arc<SessionManager>>() {
                        let _ = manager.stop_recording(&monitor_session);
                    }
                    return;
                }
                thread::sleep(Duration::from_millis(500));
            }
        });

        Ok(())
    }

    pub fn stop_recording(&self, session_id: &str) -> AppResult<SessionMetadata> {
        {
            let mut guard = self.state.lock().unwrap();
            match &*guard {
                SessionState::Recording { .. } | SessionState::Recovering { .. } => {
                    if guard.session_id() != Some(session_id) {
                        return Err(AppError::session_conflict());
                    }
                    *guard = SessionState::Stopping {
                        session_id: session_id.to_string(),
                    };
                }
                _ => return Err(AppError::session_not_found()),
            }
        }
        self.emit_state(&self.get_state());

        let active = self
            .active
            .lock()
            .unwrap()
            .take()
            .ok_or_else(AppError::session_not_found)?;

        self.engine.stop_meeting_capture();
        if active.mic_enabled {
            self.engine.stop_microphone_capture();
        }
        active.stop_flag.store(true, Ordering::Relaxed);

        self.set_state(SessionState::Finalizing {
            session_id: session_id.to_string(),
        });

        let outcome = active
            .worker
            .join()
            .map_err(|_| AppError::finalize_failed())?;

        let ended_at = Utc::now();
        let duration_seconds =
            outcome.meeting_frames.max(outcome.mic_frames) as f64 / CANONICAL_SAMPLE_RATE as f64;

        let metadata = SessionMetadata {
            schema_version: session_store::SCHEMA_VERSION,
            session_id: active.session_id.clone(),
            started_at: active.started_at,
            ended_at,
            duration_seconds,
            mic_enabled: active.mic_enabled,
            recovered: false,
            interrupted: outcome.disk_full || outcome.write_failed,
            dropped_meeting_frames: outcome.dropped_meeting_frames,
            dropped_mic_frames: outcome.dropped_mic_frames,
            sample_rate: CANONICAL_SAMPLE_RATE,
            meeting_wav: Some("meeting.wav".to_string()),
            microphone_wav: if active.mic_enabled {
                Some("microphone.wav".to_string())
            } else {
                None
            },
            mixed_wav: "mixed.wav".to_string(),
            source_app: active.source_app,
            app_bundle_id: crate::APP_BUNDLE_ID.to_string(),
            app_version: env!("CARGO_PKG_VERSION").to_string(),
        };

        session_store::write_metadata(&active.paths, &metadata)
            .map_err(|_| AppError::finalize_failed())?;
        session_store::finalize_dir(&active.paths, Local::now())
            .map_err(|_| AppError::finalize_failed())?;

        self.set_state(SessionState::Completed {
            metadata: metadata.clone(),
        });

        self.notify_session_finalized(&metadata.session_id);

        Ok(metadata)
    }

    /// The single boundary between recording and transcription. By the time
    /// this runs the session directory has its final name and its WAV paths
    /// resolve, which is exactly the guarantee transcription depends on -- it
    /// never sees live PCM.
    ///
    /// A no-op unless the user turned automatic transcription on and their
    /// chosen provider is ready. Nothing here can fail the recording: the
    /// files are already safely on disk.
    fn notify_session_finalized(&self, session_id: &str) {
        if let Some(service) = self
            .app_handle
            .try_state::<Arc<crate::transcription::TranscriptionService>>()
        {
            service.maybe_start_automatically(session_id);
        }
    }

    pub fn reveal_session(&self, session_id: &str) -> AppResult<()> {
        let paths = session_store::find_session_by_id(session_id)
            .map_err(|_| AppError::internal("find_session_by_id"))?
            .ok_or_else(AppError::session_not_found)?;

        // SECURITY-REVIEW: reveals a path resolved entirely server-side from a
        // generated session ID looked up on disk -- never from a
        // frontend-supplied path string.
        #[cfg(target_os = "macos")]
        {
            std::process::Command::new("open")
                .arg("-R")
                .arg(paths.mixed_wav())
                .spawn()
                .map_err(|_| AppError::internal("open -R"))?;
        }
        #[cfg(target_os = "windows")]
        {
            std::process::Command::new("explorer")
                .arg("/select,")
                .arg(paths.mixed_wav())
                .spawn()
                .map_err(|_| AppError::internal("explorer /select"))?;
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        {
            let _ = paths;
            return Err(AppError::internal("reveal_session unsupported"));
        }
        Ok(())
    }

    /// Resolves the absolute path of one track of one known session, for
    /// the embedded audio player only. This is a deliberate, narrow
    /// exception to the rule that raw filesystem paths never cross the
    /// Tauri IPC boundary: the Tauri asset protocol needs a real path, and
    /// both `session_id` and `track` are validated server-side against
    /// what's actually on disk before any path is returned.
    pub fn get_session_asset_path(&self, session_id: &str, track: &str) -> AppResult<String> {
        let paths = session_store::find_session_by_id(session_id)
            .map_err(|_| AppError::internal("find_session_by_id"))?
            .ok_or_else(AppError::session_not_found)?;
        let metadata = session_store::read_metadata(&paths)
            .map_err(|_| AppError::internal("read_metadata"))?;
        let path = session_store::resolve_track_path(&paths, &metadata, track)
            .ok_or_else(AppError::track_not_found)?;
        Ok(path.to_string_lossy().into_owned())
    }

    pub fn list_recent_sessions(&self, limit: usize) -> AppResult<Vec<SessionMetadata>> {
        session_store::list_finalized_sessions(limit)
            .map_err(|_| AppError::internal("list_finalized_sessions"))
    }

    pub fn dismiss(&self) {
        self.set_state(SessionState::Idle);
    }
}

impl SessionPaths {
    fn clone_paths(&self) -> SessionPaths {
        SessionPaths {
            dir: self.dir.clone(),
        }
    }
}

fn map_mic_start_error(code: i32) -> AppError {
    // Mirrors MicrophoneStartError in native/BlueEarAudio/.../MicrophoneCapture.swift.
    match code {
        -2 => AppError::mic_unavailable(),
        -3 => AppError::mic_permission_denied(),
        _ => AppError::internal("start_microphone_capture"),
    }
}

fn map_meeting_start_error(code: i32, app: MeetingAppId) -> AppError {
    // Mirrors ProcessTapStartError in native/BlueEarAudio/Sources/BlueEarAudio/ProcessTap.swift.
    match code {
        -2 => AppError::meeting_app_not_running(app.display_name()),
        -3 => AppError::source_silent(),
        -9 => AppError::meeting_app_not_found(app.display_name()),
        _ => AppError::audio_permission_denied(),
    }
}

fn current_platform() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "macos"
    }
    #[cfg(target_os = "windows")]
    {
        "windows"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "other"
    }
}

const MIX_LOOKAHEAD_CAP: usize = CANONICAL_SAMPLE_RATE as usize * 5;

fn write_samples_checked(
    writer: &mut CheckpointingWavWriter,
    samples: &[f32],
    io_failed: &AtomicBool,
) -> bool {
    if io_failed.load(Ordering::Relaxed) || samples.is_empty() {
        return !io_failed.load(Ordering::Relaxed);
    }
    if writer.write_samples(samples).is_err() {
        io_failed.store(true, Ordering::Relaxed);
        false
    } else {
        true
    }
}

fn process_chunk(
    chunk: &AudioChunk,
    sample_rate: u32,
    session_start_ns: &mut Option<u64>,
    writer: &mut CheckpointingWavWriter,
    pending: &mut VecDeque<f32>,
    io_failed: &AtomicBool,
) -> Option<f32> {
    if io_failed.load(Ordering::Relaxed) {
        return None;
    }
    if session_start_ns.is_none() {
        *session_start_ns = Some(chunk.host_time_ns);
    }
    let source_rate = if chunk.sample_rate > 0.0 {
        chunk.sample_rate
    } else {
        sample_rate as f64
    };
    let mono = downmix_to_mono(chunk.as_slice(), (chunk.channel_count.max(1)) as usize);
    let mono = resample_linear(&mono, source_rate, sample_rate as f64);

    if let Some(start) = *session_start_ns {
        let target = target_frame_index(start, chunk.host_time_ns, sample_rate);
        let silence = silence_frames_needed(writer.frames_written(), target);
        if silence > 0 {
            let pad = vec![0.0f32; silence as usize];
            if !write_samples_checked(writer, &pad, io_failed) {
                return None;
            }
            pending.extend(pad);
        }
        if !write_samples_checked(writer, &mono, io_failed) {
            return None;
        }
        pending.extend(mono.iter().copied());
    }

    mono.iter()
        .fold(None, |acc: Option<f32>, &s| Some(acc.map_or(s.abs(), |a| a.max(s.abs()))))
}

fn drain_mix(
    pending_meeting: &mut VecDeque<f32>,
    pending_mic: &mut VecDeque<f32>,
    mic_enabled: bool,
    mixed_writer: &mut CheckpointingWavWriter,
    io_failed: &AtomicBool,
) {
    if io_failed.load(Ordering::Relaxed) {
        return;
    }
    let n = if mic_enabled {
        pending_meeting.len().min(pending_mic.len())
    } else {
        pending_meeting.len()
    };

    if mic_enabled {
        if n > 0 {
            let mut buf = Vec::with_capacity(n);
            for _ in 0..n {
                let t = pending_meeting.pop_front().unwrap();
                let m = pending_mic.pop_front().unwrap();
                buf.push(mix_frame(Some(t), Some(m)));
            }
            if !write_samples_checked(mixed_writer, &buf, io_failed) {
                return;
            }
        }
        if pending_meeting.len() > MIX_LOOKAHEAD_CAP {
            let excess = pending_meeting.len() - MIX_LOOKAHEAD_CAP;
            let buf: Vec<f32> = (0..excess)
                .filter_map(|_| pending_meeting.pop_front())
                .map(|t| mix_frame(Some(t), None))
                .collect();
            if !write_samples_checked(mixed_writer, &buf, io_failed) {
                return;
            }
        }
        if pending_mic.len() > MIX_LOOKAHEAD_CAP {
            let excess = pending_mic.len() - MIX_LOOKAHEAD_CAP;
            let buf: Vec<f32> = (0..excess)
                .filter_map(|_| pending_mic.pop_front())
                .map(|m| mix_frame(None, Some(m)))
                .collect();
            if !write_samples_checked(mixed_writer, &buf, io_failed) {
                return;
            }
        }
    } else if !pending_meeting.is_empty() {
        let buf: Vec<f32> = pending_meeting
            .drain(..)
            .map(|t| mix_frame(Some(t), None))
            .collect();
        let _ = write_samples_checked(mixed_writer, &buf, io_failed);
    }
}

fn recording_worker(
    mut meeting_rx: RingConsumer,
    mut mic_rx: Option<RingConsumer>,
    paths: SessionPaths,
    mic_enabled: bool,
    mut meeting_writer: CheckpointingWavWriter,
    mut mic_writer: Option<CheckpointingWavWriter>,
    mut mixed_writer: CheckpointingWavWriter,
    stop_flag: Arc<AtomicBool>,
    disk_low_flag: Arc<AtomicBool>,
    io_failed_flag: Arc<AtomicBool>,
    levels: Arc<LevelMeters>,
) -> WorkerOutcome {
    let sample_rate = CANONICAL_SAMPLE_RATE;
    let mut session_start_ns: Option<u64> = None;
    let mut pending_meeting: VecDeque<f32> = VecDeque::new();
    let mut pending_mic: VecDeque<f32> = VecDeque::new();
    let mut last_checkpoint = Instant::now();
    let mut last_disk_check = Instant::now();
    let checkpoint_interval = Duration::from_secs(2);

    let should_stop = || {
        stop_flag.load(Ordering::Relaxed)
            || disk_low_flag.load(Ordering::Relaxed)
            || io_failed_flag.load(Ordering::Relaxed)
    };

    loop {
        let mut progressed = false;

        if let Some(chunk) = meeting_rx.pop() {
            progressed = true;
            if let Some(peak) = process_chunk(
                &chunk,
                sample_rate,
                &mut session_start_ns,
                &mut meeting_writer,
                &mut pending_meeting,
                &io_failed_flag,
            ) {
                levels.set_meeting(peak);
            }
        }

        if mic_enabled {
            if let Some(rx) = mic_rx.as_mut() {
                if let Some(chunk) = rx.pop() {
                    progressed = true;
                    if let Some(mic_w) = mic_writer.as_mut() {
                        if let Some(peak) = process_chunk(
                            &chunk,
                            sample_rate,
                            &mut session_start_ns,
                            mic_w,
                            &mut pending_mic,
                            &io_failed_flag,
                        ) {
                            levels.set_mic(peak);
                        }
                    }
                }
            }
        }

        drain_mix(
            &mut pending_meeting,
            &mut pending_mic,
            mic_enabled,
            &mut mixed_writer,
            &io_failed_flag,
        );

        if last_checkpoint.elapsed() >= checkpoint_interval {
            if !io_failed_flag.load(Ordering::Relaxed) && meeting_writer.checkpoint().is_err() {
                io_failed_flag.store(true, Ordering::Relaxed);
            }
            if !io_failed_flag.load(Ordering::Relaxed) {
                if let Some(w) = mic_writer.as_mut() {
                    if w.checkpoint().is_err() {
                        io_failed_flag.store(true, Ordering::Relaxed);
                    }
                }
            }
            if !io_failed_flag.load(Ordering::Relaxed) && mixed_writer.checkpoint().is_err() {
                io_failed_flag.store(true, Ordering::Relaxed);
            }
            last_checkpoint = Instant::now();
        }

        if !disk_low_flag.load(Ordering::Relaxed) && last_disk_check.elapsed() >= checkpoint_interval {
            if let Some(free) = session_store::available_bytes(&paths.dir) {
                if free < LOW_DISK_THRESHOLD_BYTES {
                    disk_low_flag.store(true, Ordering::Relaxed);
                }
            }
            last_disk_check = Instant::now();
        }

        if should_stop() && !progressed {
            break;
        }

        if !progressed {
            thread::sleep(Duration::from_millis(2));
        }
    }

    // Bounded final drain: pick up anything still sitting in the rings from
    // the moment capture stopped before it drains.
    for _ in 0..4096 {
        if should_stop() && io_failed_flag.load(Ordering::Relaxed) {
            break;
        }
        let teams_chunk = meeting_rx.pop();
        let mic_chunk = if mic_enabled {
            mic_rx.as_mut().and_then(|r| r.pop())
        } else {
            None
        };
        if teams_chunk.is_none() && mic_chunk.is_none() {
            break;
        }
        if let Some(chunk) = teams_chunk {
            if let Some(peak) = process_chunk(
                &chunk,
                sample_rate,
                &mut session_start_ns,
                &mut meeting_writer,
                &mut pending_meeting,
                &io_failed_flag,
            ) {
                levels.set_meeting(peak);
            }
        }
        if let Some(chunk) = mic_chunk {
            if let Some(mic_w) = mic_writer.as_mut() {
                if let Some(peak) = process_chunk(
                    &chunk,
                    sample_rate,
                    &mut session_start_ns,
                    mic_w,
                    &mut pending_mic,
                    &io_failed_flag,
                ) {
                    levels.set_mic(peak);
                }
            }
        }
        drain_mix(
            &mut pending_meeting,
            &mut pending_mic,
            mic_enabled,
            &mut mixed_writer,
            &io_failed_flag,
        );
    }

    // Anything left unmatched now truly has no partner arriving: flush it
    // paired with silence rather than dropping it.
    if !io_failed_flag.load(Ordering::Relaxed) {
        if mic_enabled {
            let n = pending_meeting.len().max(pending_mic.len());
            if n > 0 {
                let mut buf = Vec::with_capacity(n);
                for _ in 0..n {
                    buf.push(mix_frame(pending_meeting.pop_front(), pending_mic.pop_front()));
                }
                let _ = write_samples_checked(&mut mixed_writer, &buf, &io_failed_flag);
            }
        } else if !pending_meeting.is_empty() {
            let buf: Vec<f32> = pending_meeting
                .drain(..)
                .map(|t| mix_frame(Some(t), None))
                .collect();
            let _ = write_samples_checked(&mut mixed_writer, &buf, &io_failed_flag);
        }
    }

    let dropped_meeting_frames = meeting_rx.dropped_count();
    let dropped_mic_frames = mic_rx.as_ref().map(|r| r.dropped_count()).unwrap_or(0);
    let write_failed = io_failed_flag.load(Ordering::Relaxed);

    let meeting_frames = meeting_writer.finalize().unwrap_or(0);
    let mic_frames = if let Some(w) = mic_writer {
        w.finalize().unwrap_or(0)
    } else {
        0
    };
    let _ = mixed_writer.finalize();

    WorkerOutcome {
        meeting_frames,
        mic_frames,
        dropped_meeting_frames,
        dropped_mic_frames,
        disk_full: disk_low_flag.load(Ordering::Relaxed),
        write_failed,
    }
}

#[cfg(test)]
mod live_tests {
    //! These tests drive the real platform audio engine against
    //! whatever is actually running on the machine. They are `#[ignore]`d
    //! by default -- they need a running meeting app (Teams or Zoom),
    //! a supported OS, and (for the permission test) an already-granted
    //! capture permission for this build -- so
    //! they never run in a normal `cargo test`. Run explicitly with e.g.:
    //!
    //! ```text
    //! cargo test --release -- --ignored --nocapture live_
    //! ```

    use std::sync::atomic::Ordering;
    use std::sync::Arc;
    use std::time::Duration;

    use crate::audio::{ring, MeetingAppId, PlatformAudioEngine};

    #[test]
    #[ignore]
    fn live_os_version_and_meeting_app_detection() {
        let (engine, _status_rx) = PlatformAudioEngine::new();
        assert!(
            engine.os_supported(),
            "this machine should meet the minimum OS for capture"
        );
        let any_running = MeetingAppId::ALL
            .iter()
            .any(|&app| engine.is_meeting_app_running(app));
        assert!(
            any_running,
            "start Microsoft Teams or Zoom before running this test"
        );
    }

    #[test]
    #[ignore]
    fn live_teams_capture_produces_real_frames() {
        live_meeting_capture(MeetingAppId::Teams);
    }

    #[test]
    #[ignore]
    fn live_zoom_capture_produces_real_frames() {
        live_meeting_capture(MeetingAppId::Zoom);
    }

    fn live_meeting_capture(app: MeetingAppId) {
        let (engine, _status_rx) = PlatformAudioEngine::new();
        assert!(
            engine.is_meeting_app_running(app),
            "start {} before running this test",
            app.display_name()
        );

        let (producer, mut consumer) = ring::channel();
        engine
            .start_meeting_capture(app, producer)
            .expect("failed to start meeting process-tap capture");

        // Drain concurrently on a background thread, like the real
        // `recording_worker` does, instead of sleeping and draining
        // afterwards -- the ring only holds ~0.5s of audio, so draining
        // after the fact would just measure how fast the ring overflows.
        let stop_draining = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_draining_worker = stop_draining.clone();
        let drain_handle = std::thread::spawn(move || {
            let mut total_frames: u64 = 0;
            let mut peak = 0f32;
            loop {
                match consumer.pop() {
                    Some(chunk) => {
                        total_frames += chunk.frame_count as u64;
                        for &s in chunk.as_slice() {
                            peak = peak.max(s.abs());
                        }
                    }
                    None => {
                        if stop_draining_worker.load(Ordering::Relaxed) {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
            }
            (total_frames, peak, consumer.dropped_count())
        });

        std::thread::sleep(Duration::from_secs(5));
        stop_draining.store(true, Ordering::Relaxed);
        let (total_frames, peak, dropped) = drain_handle.join().expect("drain thread panicked");

        engine.stop_meeting_capture();

        println!(
            "live_{}_capture_produces_real_frames: total_frames={total_frames} peak={peak} dropped={dropped}",
            app.wire_id()
        );
        assert_eq!(
            dropped, 0,
            "ring buffer dropped chunks even with continuous draining; \
             the recording worker's consumer loop may be too slow"
        );
        assert!(
            total_frames > 0,
            "expected at least some frames from the {} tap; got 0. \
             This is the documented signature of a missing/denied system-audio \
             (kTCCServiceAudioCapture) permission grant for this build's signature.",
            app.display_name()
        );
    }
}
