//! Session directory layout, metadata persistence, and disk-space checks.
//!
//! All paths are resolved server-side from generated session IDs or from
//! metadata already on disk -- no user-supplied path is ever accepted from
//! the frontend, matching the plan's Tauri API contract.
//!
//! Schema version 2 renames the meeting track to `meeting.wav` / `meetingWav`
//! and records `sourceApp`. v1 sessions that wrote `teams.wav` still load via
//! serde aliases and filename resolution from metadata.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Local, Utc};
use serde::{Deserialize, Serialize};

use crate::audio::MeetingAppId;

pub const SCHEMA_VERSION: u32 = 2;
const IN_PROGRESS_PREFIX: &str = ".inprogress-";
const MEETING_WAV_NAME: &str = "meeting.wav";
const LEGACY_TEAMS_WAV_NAME: &str = "teams.wav";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMetadata {
    pub schema_version: u32,
    pub session_id: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: DateTime<Utc>,
    pub duration_seconds: f64,
    pub mic_enabled: bool,
    pub recovered: bool,
    pub interrupted: bool,
    #[serde(alias = "droppedTeamsFrames")]
    pub dropped_meeting_frames: u64,
    pub dropped_mic_frames: u64,
    pub sample_rate: u32,
    #[serde(alias = "teamsWav")]
    pub meeting_wav: Option<String>,
    pub microphone_wav: Option<String>,
    pub mixed_wav: String,
    /// Which meeting app produced the meeting track. Missing on v1 files;
    /// defaults to Teams when deserializing.
    #[serde(default)]
    pub source_app: MeetingAppId,
    pub app_bundle_id: String,
    pub app_version: String,
}

/// Tiny marker written at session start so crash recovery knows which app
/// owned an `.inprogress-*` directory before `session.json` existed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceMarker {
    pub source_app: MeetingAppId,
}

pub fn recordings_root() -> PathBuf {
    crate::paths::recordings_root()
}

#[derive(Debug, Clone)]
pub struct SessionPaths {
    pub dir: PathBuf,
}

impl SessionPaths {
    /// Path used when *creating* a new session's meeting track.
    pub fn meeting_wav(&self) -> PathBuf {
        self.dir.join(MEETING_WAV_NAME)
    }

    /// Resolve the on-disk meeting WAV for an existing session from metadata
    /// (supports legacy `teams.wav` filenames).
    pub fn meeting_wav_for(&self, metadata: &SessionMetadata) -> PathBuf {
        match metadata.meeting_wav.as_deref() {
            Some(name) => self.dir.join(name),
            None => self.meeting_wav(),
        }
    }

    pub fn microphone_wav(&self) -> PathBuf {
        self.dir.join("microphone.wav")
    }
    pub fn mixed_wav(&self) -> PathBuf {
        self.dir.join("mixed.wav")
    }
    pub fn metadata_json(&self) -> PathBuf {
        self.dir.join("session.json")
    }
    pub fn source_json(&self) -> PathBuf {
        self.dir.join("source.json")
    }
}

pub fn create_in_progress_dir(session_id: &str, source_app: MeetingAppId) -> io::Result<SessionPaths> {
    let root = recordings_root();
    fs::create_dir_all(&root)?;
    let dir = root.join(format!("{IN_PROGRESS_PREFIX}{session_id}"));
    fs::create_dir_all(&dir)?;
    let paths = SessionPaths { dir };
    write_source_marker(&paths, source_app)?;
    Ok(paths)
}

pub fn write_source_marker(paths: &SessionPaths, source_app: MeetingAppId) -> io::Result<()> {
    let marker = SourceMarker { source_app };
    let json = serde_json::to_vec_pretty(&marker)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(paths.source_json(), json)
}

pub fn read_source_marker(paths: &SessionPaths) -> Option<MeetingAppId> {
    let bytes = fs::read(paths.source_json()).ok()?;
    if let Ok(marker) = serde_json::from_slice::<SourceMarker>(&bytes) {
        return Some(marker.source_app);
    }
    // Tolerate a bare wire id (e.g. `"zoom"`) from hand-edited markers.
    let text = String::from_utf8_lossy(&bytes);
    text.trim().trim_matches('"').parse().ok()
}

/// Renames a `.inprogress-<uuid>` directory to a stable, human-readable
/// `YYYY-MM-DD_HH-mm-ss[-N]` name once recording has stopped successfully.
pub fn finalize_dir(in_progress: &SessionPaths, started_at: DateTime<Local>) -> io::Result<SessionPaths> {
    let root = recordings_root();
    let base_name = started_at.format("%Y-%m-%d_%H-%M-%S").to_string();
    let mut final_dir = root.join(&base_name);
    let mut suffix = 1u32;
    while final_dir.exists() {
        final_dir = root.join(format!("{base_name}-{suffix}"));
        suffix += 1;
    }
    fs::rename(&in_progress.dir, &final_dir)?;
    Ok(SessionPaths { dir: final_dir })
}

pub fn write_metadata(paths: &SessionPaths, metadata: &SessionMetadata) -> io::Result<()> {
    let json = serde_json::to_vec_pretty(metadata)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(paths.metadata_json(), json)
}

pub fn read_metadata(paths: &SessionPaths) -> io::Result<SessionMetadata> {
    let bytes = fs::read(paths.metadata_json())?;
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Every finalized session directory, newest first. Separate from
/// [`list_finalized_sessions`] because transcription recovery needs the
/// directories themselves (to find job files) rather than parsed metadata,
/// and must not skip a session whose `session.json` happens to be unreadable.
pub fn list_finalized_session_dirs() -> io::Result<Vec<PathBuf>> {
    let root = recordings_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<_> = fs::read_dir(&root)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter(|e| !e.file_name().to_string_lossy().starts_with(IN_PROGRESS_PREFIX))
        .collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok()));
    Ok(entries.into_iter().map(|e| e.path()).collect())
}

pub fn list_finalized_sessions(limit: usize) -> io::Result<Vec<SessionMetadata>> {
    let root = recordings_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<_> = fs::read_dir(&root)?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter(|e| !e.file_name().to_string_lossy().starts_with(IN_PROGRESS_PREFIX))
        .collect();
    entries.sort_by_key(|e| {
        std::cmp::Reverse(e.metadata().and_then(|m| m.modified()).ok())
    });

    Ok(entries
        .into_iter()
        .take(limit)
        .filter_map(|entry| {
            let paths = SessionPaths { dir: entry.path() };
            read_metadata(&paths).ok()
        })
        .collect())
}

/// Resolves the on-disk path for one track of an already-located session,
/// or `None` if that track doesn't exist for this session. Track names
/// `"meeting"` and legacy `"teams"` both resolve the meeting WAV.
pub fn resolve_track_path(
    paths: &SessionPaths,
    metadata: &SessionMetadata,
    track: &str,
) -> Option<PathBuf> {
    match track {
        "meeting" | "teams" if metadata.meeting_wav.is_some() => {
            Some(paths.meeting_wav_for(metadata))
        }
        "microphone" if metadata.microphone_wav.is_some() => Some(paths.microphone_wav()),
        "mixed" => Some(paths.mixed_wav()),
        _ => None,
    }
}

pub fn find_session_by_id(session_id: &str) -> io::Result<Option<SessionPaths>> {
    let root = recordings_root();
    if !root.exists() {
        return Ok(None);
    }
    for entry in fs::read_dir(&root)?.filter_map(|e| e.ok()) {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let paths = SessionPaths { dir: path };
        if let Ok(meta) = read_metadata(&paths) {
            if meta.session_id == session_id {
                return Ok(Some(paths));
            }
        }
    }
    Ok(None)
}

pub fn list_in_progress_dirs() -> io::Result<Vec<PathBuf>> {
    let root = recordings_root();
    if !root.exists() {
        return Ok(Vec::new());
    }
    Ok(fs::read_dir(&root)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .map(|n| n.to_string_lossy().starts_with(IN_PROGRESS_PREFIX))
                    .unwrap_or(false)
        })
        .collect())
}

/// Available disk space at (or above) `path`, in bytes. Returns `None` if it
/// cannot be determined rather than failing the caller.
pub fn available_bytes(path: &Path) -> Option<u64> {
    let probe_path = if path.exists() {
        path.to_path_buf()
    } else {
        path.parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(if cfg!(windows) { "C:\\" } else { "/" }))
    };

    available_bytes_impl(&probe_path)
}

#[cfg(unix)]
fn available_bytes_impl(probe_path: &Path) -> Option<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(probe_path.as_os_str().as_bytes()).ok()?;
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut stat) == 0 {
            Some(stat.f_bavail as u64 * stat.f_frsize as u64)
        } else {
            None
        }
    }
}

#[cfg(windows)]
fn available_bytes_impl(probe_path: &Path) -> Option<u64> {
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let wide: Vec<u16> = probe_path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    unsafe {
        let mut free_bytes_available = 0u64;
        GetDiskFreeSpaceExW(
            PCWSTR(wide.as_ptr()),
            Some(&mut free_bytes_available),
            None,
            None,
        )
        .ok()?;
        Some(free_bytes_available)
    }
}

#[cfg(not(any(unix, windows)))]
fn available_bytes_impl(_probe_path: &Path) -> Option<u64> {
    None
}

/// Detect which meeting WAV filename is present in an in-progress or recovered dir.
pub fn detect_meeting_wav_filename(paths: &SessionPaths) -> Option<&'static str> {
    if paths.meeting_wav().exists() {
        Some(MEETING_WAV_NAME)
    } else if paths.dir.join(LEGACY_TEAMS_WAV_NAME).exists() {
        Some(LEGACY_TEAMS_WAV_NAME)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::HOME_ENV_LOCK;
    use tempfile::tempdir;

    #[test]
    fn finalize_dir_avoids_collisions() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempdir().unwrap();
        std::env::set_var("HOME", dir.path());
        let started_at = Local::now();

        let a = create_in_progress_dir("session-a", MeetingAppId::Teams).unwrap();
        let final_a = finalize_dir(&a, started_at).unwrap();

        let b = create_in_progress_dir("session-b", MeetingAppId::Zoom).unwrap();
        let final_b = finalize_dir(&b, started_at).unwrap();

        assert_ne!(final_a.dir, final_b.dir);
        assert!(final_a.dir.exists());
        assert!(final_b.dir.exists());
    }

    #[test]
    fn write_and_read_metadata_round_trips() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempdir().unwrap();
        std::env::set_var("HOME", dir.path());
        let paths = create_in_progress_dir("session-c", MeetingAppId::Zoom).unwrap();
        let metadata = SessionMetadata {
            schema_version: SCHEMA_VERSION,
            session_id: "session-c".into(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            duration_seconds: 12.5,
            mic_enabled: true,
            recovered: false,
            interrupted: false,
            dropped_meeting_frames: 0,
            dropped_mic_frames: 0,
            sample_rate: 48_000,
            meeting_wav: Some("meeting.wav".into()),
            microphone_wav: Some("microphone.wav".into()),
            mixed_wav: "mixed.wav".into(),
            source_app: MeetingAppId::Zoom,
            app_bundle_id: "com.blueear.app".into(),
            app_version: "0.1.0".into(),
        };
        write_metadata(&paths, &metadata).unwrap();
        let read_back = read_metadata(&paths).unwrap();
        assert_eq!(read_back.session_id, "session-c");
        assert_eq!(read_back.source_app, MeetingAppId::Zoom);
        assert!((read_back.duration_seconds - 12.5).abs() < 1e-9);
    }

    #[test]
    fn v1_teams_metadata_deserializes_with_aliases() {
        let json = r#"{
            "schemaVersion": 1,
            "sessionId": "legacy",
            "startedAt": "2026-01-01T00:00:00Z",
            "endedAt": "2026-01-01T00:01:00Z",
            "durationSeconds": 60.0,
            "micEnabled": false,
            "recovered": false,
            "interrupted": false,
            "droppedTeamsFrames": 3,
            "droppedMicFrames": 0,
            "sampleRate": 48000,
            "teamsWav": "teams.wav",
            "microphoneWav": null,
            "mixedWav": "mixed.wav",
            "appBundleId": "com.blueear.app",
            "appVersion": "0.1.0"
        }"#;
        let meta: SessionMetadata = serde_json::from_str(json).unwrap();
        assert_eq!(meta.meeting_wav.as_deref(), Some("teams.wav"));
        assert_eq!(meta.dropped_meeting_frames, 3);
        assert_eq!(meta.source_app, MeetingAppId::Teams);
    }

    fn sample_metadata(session_id: &str, mic_enabled: bool) -> SessionMetadata {
        SessionMetadata {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.into(),
            started_at: Utc::now(),
            ended_at: Utc::now(),
            duration_seconds: 5.0,
            mic_enabled,
            recovered: false,
            interrupted: false,
            dropped_meeting_frames: 0,
            dropped_mic_frames: 0,
            sample_rate: 48_000,
            meeting_wav: Some("meeting.wav".into()),
            microphone_wav: mic_enabled.then(|| "microphone.wav".into()),
            mixed_wav: "mixed.wav".into(),
            source_app: MeetingAppId::Teams,
            app_bundle_id: "com.blueear.app".into(),
            app_version: "0.1.0".into(),
        }
    }

    #[test]
    fn resolve_track_path_returns_mixed_track_unconditionally() {
        let paths = SessionPaths { dir: PathBuf::from("/tmp/session") };
        let metadata = sample_metadata("session-mixed", false);
        assert_eq!(
            resolve_track_path(&paths, &metadata, "mixed"),
            Some(paths.mixed_wav())
        );
    }

    #[test]
    fn resolve_track_path_returns_none_for_track_never_recorded() {
        let paths = SessionPaths { dir: PathBuf::from("/tmp/session") };
        let metadata = sample_metadata("session-no-mic", false);
        assert_eq!(resolve_track_path(&paths, &metadata, "microphone"), None);
        assert_eq!(
            resolve_track_path(&paths, &metadata, "meeting"),
            Some(paths.meeting_wav())
        );
        assert_eq!(
            resolve_track_path(&paths, &metadata, "teams"),
            Some(paths.meeting_wav())
        );
    }

    #[test]
    fn resolve_track_path_returns_none_for_unknown_track_name() {
        let paths = SessionPaths { dir: PathBuf::from("/tmp/session") };
        let metadata = sample_metadata("session-unknown-track", true);
        assert_eq!(resolve_track_path(&paths, &metadata, "video"), None);
    }

    #[test]
    fn resolve_track_path_returns_microphone_when_enabled() {
        let paths = SessionPaths { dir: PathBuf::from("/tmp/session") };
        let metadata = sample_metadata("session-with-mic", true);
        assert_eq!(
            resolve_track_path(&paths, &metadata, "microphone"),
            Some(paths.microphone_wav())
        );
    }

    #[test]
    fn source_marker_round_trips() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempdir().unwrap();
        std::env::set_var("HOME", dir.path());
        let paths = create_in_progress_dir("session-marker", MeetingAppId::Zoom).unwrap();
        assert_eq!(read_source_marker(&paths), Some(MeetingAppId::Zoom));
    }

    #[test]
    fn find_session_by_id_returns_none_for_unknown_session() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempdir().unwrap();
        std::env::set_var("HOME", dir.path());

        let paths = create_in_progress_dir("session-known", MeetingAppId::Teams).unwrap();
        let metadata = sample_metadata("session-known", false);
        write_metadata(&paths, &metadata).unwrap();
        finalize_dir(&paths, Local::now()).unwrap();

        assert!(find_session_by_id("session-does-not-exist").unwrap().is_none());
        assert!(find_session_by_id("session-known").unwrap().is_some());
    }
}
