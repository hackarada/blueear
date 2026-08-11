//! Atomic on-disk persistence for transcripts, jobs, and preferences, plus the
//! derived text and VTT exports.
//!
//! Everything here writes through a temporary file and a rename, because a
//! transcription job can be minutes long and the app can be force-quit at any
//! point in it. A half-written `transcript.json` would be worse than no
//! transcript at all: the next launch would read it, fail to parse it, and the
//! user's only recourse would be to rerun the whole job.
//!
//! Paths are derived server-side from a session ID or from a fixed
//! application-support root. Nothing here accepts a path from the frontend.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::storage::session_store::SessionPaths;
use crate::transcription::types::{Job, JobStatus, ProviderId, Transcript};

pub const PREFERENCES_SCHEMA_VERSION: u32 = 1;

const TRANSCRIPT_FILE: &str = "transcript.json";
const JOB_FILE: &str = "transcription-job.json";
const PREFERENCES_FILE: &str = "transcription-preferences.json";

/// User-chosen transcription settings, global rather than per-session.
///
/// The defaults matter: a fresh install must be a recorder and nothing else,
/// so the provider is `none` and auto-transcribe is off. Importing models does
/// not change either of them -- turning transcription on stays an explicit act.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    pub schema_version: u32,
    pub provider: ProviderId,
    pub auto_transcribe: bool,
    /// Only consulted when the selected provider supports diarization.
    pub diarize_remote_speakers: bool,
    /// BCP-47 tag, or `None` to let the provider decide.
    pub language: Option<String>,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            schema_version: PREFERENCES_SCHEMA_VERSION,
            provider: ProviderId::None,
            auto_transcribe: false,
            diarize_remote_speakers: true,
            language: None,
        }
    }
}

/// Application support directory for preferences and model bundles.
/// Resolved via [`crate::paths`] so tests can redirect with `HOME`.
pub fn app_support_root() -> PathBuf {
    crate::paths::app_support_root()
}

/// Where validated model bundles are promoted to. Providers load only from
/// here.
pub fn models_root() -> PathBuf {
    app_support_root().join("Models")
}

// MARK: - Atomic writes

/// Serializes to a sibling temporary file, flushes it, then renames over the
/// target. `rename` within one directory is atomic on APFS, so a reader either
/// sees the previous complete file or the new complete file, never a partial
/// one.
fn write_json_atomically<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    use std::io::Write;

    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?;
    fs::create_dir_all(parent)?;

    let temp_path = parent.join(format!(
        ".{}.tmp",
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "write".to_string())
    ));

    let json = serde_json::to_vec_pretty(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(&json)?;
        file.sync_all()?;
    }
    fs::rename(&temp_path, path)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> io::Result<T> {
    let bytes = fs::read(path)?;
    serde_json::from_slice(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

// MARK: - Session-scoped files

pub fn transcript_path(paths: &SessionPaths) -> PathBuf {
    paths.dir.join(TRANSCRIPT_FILE)
}

pub fn job_path(paths: &SessionPaths) -> PathBuf {
    paths.dir.join(JOB_FILE)
}

pub fn write_transcript(paths: &SessionPaths, transcript: &Transcript) -> io::Result<()> {
    write_json_atomically(&transcript_path(paths), transcript)
}

pub fn read_transcript(paths: &SessionPaths) -> io::Result<Option<Transcript>> {
    let path = transcript_path(paths);
    if !path.exists() {
        return Ok(None);
    }
    read_json(&path).map(Some)
}

pub fn write_job(paths: &SessionPaths, job: &Job) -> io::Result<()> {
    write_json_atomically(&job_path(paths), job)
}

pub fn read_job(paths: &SessionPaths) -> io::Result<Option<Job>> {
    let path = job_path(paths);
    if !path.exists() {
        return Ok(None);
    }
    read_json(&path).map(Some)
}

// MARK: - Preferences

pub fn preferences_path() -> PathBuf {
    app_support_root().join(PREFERENCES_FILE)
}

/// Reads preferences, falling back to the recorder-only defaults if the file is
/// absent or unreadable. A corrupt preferences file must never prevent the app
/// from starting, and defaulting to `none` is the safe direction to fail in.
pub fn read_preferences() -> Preferences {
    read_json(&preferences_path()).unwrap_or_default()
}

pub fn write_preferences(preferences: &Preferences) -> io::Result<()> {
    write_json_atomically(&preferences_path(), preferences)
}

// MARK: - Recovery

/// Marks jobs that were still running when the app quit as failed-but-
/// retryable, preserving whatever per-track results were already persisted.
///
/// Deliberately does not resume inference. Silently spending minutes of CPU on
/// a job the user did not ask for on this launch would be a surprising way to
/// greet someone who just reopened the app; the UI offers Retry instead.
/// Returns how many jobs were recovered.
pub fn recover_interrupted_jobs() -> usize {
    let Ok(sessions) = crate::storage::session_store::list_finalized_session_dirs() else {
        return 0;
    };

    let mut recovered = 0;
    for dir in sessions {
        let paths = SessionPaths { dir };
        let Ok(Some(mut job)) = read_job(&paths) else {
            continue;
        };
        if job.status.is_terminal() {
            continue;
        }

        job.status = JobStatus::Failed;
        job.error = Some(crate::error::AppError::transcription_interrupted());
        for track in &mut job.tracks {
            if !track.status.is_terminal() {
                track.status = JobStatus::Failed;
                track.error = Some(crate::error::AppError::transcription_interrupted());
            }
        }
        job.refresh();

        if write_job(&paths, &job).is_ok() {
            recovered += 1;
        }
    }
    recovered
}

/// Removes model-bundle staging directories left behind by an import that was
/// interrupted. See `model_import.rs` for why imports stage before promoting.
pub fn clean_model_staging_dirs() {
    let root = models_root();
    let Ok(entries) = fs::read_dir(&root) else {
        return;
    };
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(".staging-") {
            let _ = fs::remove_dir_all(entry.path());
        }
    }
}

// MARK: - Exports

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExportFormat {
    Text,
    Vtt,
}

impl ExportFormat {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "text" => Some(ExportFormat::Text),
            "vtt" => Some(ExportFormat::Vtt),
            _ => None,
        }
    }

    fn file_name(self) -> &'static str {
        match self {
            ExportFormat::Text => "transcript.txt",
            ExportFormat::Vtt => "transcript.vtt",
        }
    }
}

/// Renders and writes an export beside the recording, returning its path.
pub fn write_export(
    paths: &SessionPaths,
    transcript: &Transcript,
    format: ExportFormat,
) -> io::Result<PathBuf> {
    let rendered = match format {
        ExportFormat::Text => render_text(transcript),
        ExportFormat::Vtt => render_vtt(transcript),
    };
    let path = paths.dir.join(format.file_name());
    fs::write(&path, rendered)?;
    Ok(path)
}

pub fn render_text(transcript: &Transcript) -> String {
    let mut out = String::new();
    for segment in &transcript.segments {
        out.push_str(&format!(
            "[{}] {}: {}\n",
            format_clock(segment.start_seconds),
            segment.speaker.display_name(),
            segment.text
        ));
    }
    out
}

pub fn render_vtt(transcript: &Transcript) -> String {
    let mut out = String::from("WEBVTT\n\n");
    for (index, segment) in transcript.segments.iter().enumerate() {
        out.push_str(&format!(
            "{}\n{} --> {}\n<v {}>{}\n\n",
            index + 1,
            format_timestamp(segment.start_seconds),
            format_timestamp(segment.end_seconds),
            segment.speaker.display_name(),
            segment.text
        ));
    }
    out
}

fn format_clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// `HH:MM:SS.mmm`, as the WebVTT spec requires.
fn format_timestamp(seconds: f64) -> String {
    let clamped = seconds.max(0.0);
    let total_ms = (clamped * 1000.0).round() as u64;
    let ms = total_ms % 1000;
    let total_seconds = total_ms / 1000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        total_seconds / 3600,
        (total_seconds % 3600) / 60,
        total_seconds % 60,
        ms
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::HOME_ENV_LOCK;
    use crate::transcription::types::{
        Segment, Speaker, Track, TrackTranscript, TRANSCRIPT_SCHEMA_VERSION,
    };
    use chrono::Utc;
    use tempfile::tempdir;

    fn sample_transcript(session_id: &str) -> Transcript {
        let segments = vec![
            Segment {
                track: Track::Meeting,
                speaker: Speaker::Remote { index: 1 },
                start_seconds: 0.0,
                end_seconds: 1.5,
                text: "good morning".into(),
                words: vec![],
            },
            Segment {
                track: Track::Microphone,
                speaker: Speaker::You,
                start_seconds: 2.0,
                end_seconds: 3.25,
                text: "morning".into(),
                words: vec![],
            },
        ];
        Transcript {
            schema_version: TRANSCRIPT_SCHEMA_VERSION,
            session_id: session_id.to_string(),
            provider: ProviderId::FluidAudio,
            created_at: Utc::now(),
            duration_seconds: 3.25,
            tracks: vec![TrackTranscript {
                track: Track::Meeting,
                provider: ProviderId::FluidAudio,
                model_id: None,
                language: None,
                diarized: true,
                speaker_count: 1,
                segments: segments[..1].to_vec(),
            }],
            segments,
        }
    }

    #[test]
    fn transcript_round_trips_through_disk() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempdir().unwrap();
        let paths = SessionPaths {
            dir: dir.path().to_path_buf(),
        };

        assert!(read_transcript(&paths).unwrap().is_none());
        let transcript = sample_transcript("session-a");
        write_transcript(&paths, &transcript).unwrap();
        assert_eq!(read_transcript(&paths).unwrap().unwrap(), transcript);
    }

    #[test]
    fn atomic_write_leaves_no_temporary_file_behind() {
        let dir = tempdir().unwrap();
        let paths = SessionPaths {
            dir: dir.path().to_path_buf(),
        };
        write_transcript(&paths, &sample_transcript("session-b")).unwrap();

        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp file was not renamed away");
    }

    #[test]
    fn preferences_default_to_recording_only() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempdir().unwrap();
        std::env::set_var("HOME", dir.path());

        let prefs = read_preferences();
        assert_eq!(prefs.provider, ProviderId::None);
        assert!(!prefs.auto_transcribe);
    }

    #[test]
    fn a_corrupt_preferences_file_falls_back_to_defaults_instead_of_failing() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempdir().unwrap();
        std::env::set_var("HOME", dir.path());
        fs::create_dir_all(app_support_root()).unwrap();
        fs::write(preferences_path(), b"{ not json").unwrap();

        assert_eq!(read_preferences().provider, ProviderId::None);
    }

    #[test]
    fn preferences_round_trip() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempdir().unwrap();
        std::env::set_var("HOME", dir.path());

        let prefs = Preferences {
            provider: ProviderId::FluidAudio,
            auto_transcribe: true,
            diarize_remote_speakers: false,
            language: Some("en-GB".into()),
            ..Preferences::default()
        };
        write_preferences(&prefs).unwrap();
        assert_eq!(read_preferences(), prefs);
    }

    #[test]
    fn text_export_labels_every_segment_with_a_speaker_and_a_clock() {
        let rendered = render_text(&sample_transcript("session-c"));
        assert_eq!(
            rendered,
            "[00:00] Speaker 1: good morning\n[00:02] You: morning\n"
        );
    }

    #[test]
    fn vtt_export_uses_spec_timestamps_and_voice_spans() {
        let rendered = render_vtt(&sample_transcript("session-d"));
        assert!(rendered.starts_with("WEBVTT\n\n"));
        assert!(rendered.contains("00:00:00.000 --> 00:00:01.500"));
        assert!(rendered.contains("<v Speaker 1>good morning"));
        assert!(rendered.contains("<v You>morning"));
    }

    #[test]
    fn vtt_timestamps_cross_the_hour_boundary_correctly() {
        assert_eq!(format_timestamp(3661.5), "01:01:01.500");
        assert_eq!(format_timestamp(-4.0), "00:00:00.000");
    }
}
