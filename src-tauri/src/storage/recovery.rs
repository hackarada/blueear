//! Startup recovery for sessions interrupted by a crash or forced quit.
//!
//! Any directory still named `.inprogress-<id>` when the app launches means
//! `stop_recording` never finished. This repairs each WAV header to match
//! the bytes actually on disk (never touching the audio bytes themselves),
//! writes best-effort metadata marked `recovered: true, interrupted: true`,
//! and renames the directory into the normal finalized layout so it shows
//! up in the recordings list.

use std::path::Path;

use chrono::{DateTime, Local, Utc};

use crate::audio::{MeetingAppId, CANONICAL_SAMPLE_RATE};
use crate::storage::session_store::{self, SessionMetadata, SessionPaths};
use crate::storage::wav_writer;

pub fn recover_incomplete_sessions() {
    let dirs = match session_store::list_in_progress_dirs() {
        Ok(dirs) => dirs,
        Err(e) => {
            log::warn!("could not list in-progress sessions: {e}");
            return;
        }
    };
    for dir in dirs {
        if let Err(e) = recover_one(&dir) {
            log::warn!("failed to recover interrupted session: {e}");
        }
    }
}

fn recover_one(dir: &Path) -> std::io::Result<()> {
    let paths = SessionPaths {
        dir: dir.to_path_buf(),
    };

    let mic_enabled = paths.microphone_wav().exists();
    let meeting_filename = session_store::detect_meeting_wav_filename(&paths);
    let meeting_present = meeting_filename.is_some();
    let mixed_present = paths.mixed_wav().exists();

    if !mixed_present && !meeting_present {
        // Nothing meaningful was ever captured (e.g. crashed during
        // Preparing before any writer was created). Leave the directory
        // as-is rather than fabricating a zero-length "recording".
        return Ok(());
    }

    let meeting_frames = if let Some(name) = meeting_filename {
        wav_writer::repair_header(&paths.dir.join(name), CANONICAL_SAMPLE_RATE)?
    } else {
        0
    };
    let mic_frames = if mic_enabled {
        wav_writer::repair_header(&paths.microphone_wav(), CANONICAL_SAMPLE_RATE)?
    } else {
        0
    };
    let mixed_frames = if mixed_present {
        wav_writer::repair_header(&paths.mixed_wav(), CANONICAL_SAMPLE_RATE)?
    } else {
        0
    };

    let frames = meeting_frames.max(mic_frames).max(mixed_frames);
    let duration_seconds = frames as f64 / CANONICAL_SAMPLE_RATE as f64;

    let started_at: DateTime<Utc> = std::fs::metadata(dir)
        .and_then(|m| m.modified())
        .map(DateTime::<Utc>::from)
        .unwrap_or_else(|_| Utc::now());

    let session_id = dir
        .file_name()
        .and_then(|n| n.to_str())
        .and_then(|n| n.strip_prefix(".inprogress-"))
        .unwrap_or("unknown")
        .to_string();

    // Prefer the source marker written at session start; pre-migration
    // in-progress dirs without one default to Teams.
    let source_app = session_store::read_source_marker(&paths).unwrap_or(MeetingAppId::Teams);

    let metadata = SessionMetadata {
        schema_version: session_store::SCHEMA_VERSION,
        session_id,
        started_at,
        ended_at: Utc::now(),
        duration_seconds,
        mic_enabled,
        recovered: true,
        interrupted: true,
        dropped_meeting_frames: 0,
        dropped_mic_frames: 0,
        sample_rate: CANONICAL_SAMPLE_RATE,
        meeting_wav: meeting_filename.map(|s| s.to_string()),
        microphone_wav: if mic_enabled {
            Some("microphone.wav".to_string())
        } else {
            None
        },
        mixed_wav: "mixed.wav".to_string(),
        source_app,
        app_bundle_id: crate::APP_BUNDLE_ID.to_string(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
    };

    session_store::write_metadata(&paths, &metadata)?;
    session_store::finalize_dir(&paths, started_at.with_timezone(&Local))?;
    log::info!("recovered interrupted session ({duration_seconds:.1}s, source={source_app})");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::wav_writer::{self, CheckpointingWavWriter};
    use crate::test_support::HOME_ENV_LOCK;
    use tempfile::tempdir;

    /// Simulates a forced-quit (`kill -9`) style crash mid-recording: a
    /// `.inprogress-<uuid>` directory with WAV files that were written to
    /// but never checkpointed or finalized, and no `session.json`. This is
    /// exactly the on-disk state `stop_recording` never got to clean up.
    /// Startup recovery must repair it into a normal finalized session
    /// without losing or corrupting any of the captured audio bytes.
    #[test]
    fn forced_exit_session_is_repaired_and_finalized_on_next_launch() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let home = tempdir().unwrap();
        std::env::set_var("HOME", home.path());

        let paths =
            session_store::create_in_progress_dir("crash-test-session", MeetingAppId::Zoom)
                .unwrap();
        {
            // Deliberately dropped without checkpoint/finalize.
            let mut meeting =
                CheckpointingWavWriter::create(&paths.meeting_wav(), 48_000).unwrap();
            meeting.write_samples(&vec![0.5; 4800]).unwrap();
            let mut mixed = CheckpointingWavWriter::create(&paths.mixed_wav(), 48_000).unwrap();
            mixed.write_samples(&vec![0.5; 4800]).unwrap();
        }
        // No microphone.wav and no session.json -- matches a mic-off
        // recording that crashed before `stop_recording` ran.
        assert!(!paths.microphone_wav().exists());
        assert!(!paths.metadata_json().exists());

        recover_incomplete_sessions();

        // The `.inprogress-*` directory must be gone, replaced by a
        // normal, human-readable finalized directory.
        assert!(
            session_store::list_in_progress_dirs().unwrap().is_empty(),
            "no in-progress directories should remain after recovery"
        );

        let sessions = session_store::list_finalized_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1, "exactly one finalized session expected");
        let recovered = &sessions[0];

        assert!(recovered.recovered, "session must be flagged recovered");
        assert!(recovered.interrupted, "session must be flagged interrupted");
        assert!(!recovered.mic_enabled);
        assert_eq!(recovered.session_id, "crash-test-session");
        assert_eq!(recovered.source_app, MeetingAppId::Zoom);

        // Audio bytes themselves must be untouched -- only the header was
        // repaired to reflect what was actually written before the crash.
        let final_paths = session_store::find_session_by_id("crash-test-session")
            .unwrap()
            .expect("recovered session should be discoverable by id");
        let meeting_samples =
            wav_writer::read_all_samples(&final_paths.meeting_wav_for(recovered)).unwrap();
        assert_eq!(meeting_samples.len(), 4800);
        assert!(meeting_samples.iter().all(|&s| (s - 0.5).abs() < 1e-6));
    }

    /// A directory that crashed before any writer ever created a WAV file
    /// (e.g. during `Preparing`) has nothing meaningful to recover. It must
    /// be left alone rather than turned into a fabricated empty recording.
    #[test]
    fn empty_in_progress_dir_is_left_untouched() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let home = tempdir().unwrap();
        std::env::set_var("HOME", home.path());

        session_store::create_in_progress_dir("never-started", MeetingAppId::Teams).unwrap();

        recover_incomplete_sessions();

        assert_eq!(session_store::list_in_progress_dirs().unwrap().len(), 1);
        assert!(session_store::list_finalized_sessions(10).unwrap().is_empty());
    }

    #[test]
    fn legacy_teams_wav_filename_is_recovered() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let home = tempdir().unwrap();
        std::env::set_var("HOME", home.path());

        // Simulate a pre-migration crash dir that has teams.wav and no source.json.
        let root = session_store::recordings_root();
        std::fs::create_dir_all(&root).unwrap();
        let dir = root.join(".inprogress-legacy-teams");
        std::fs::create_dir_all(&dir).unwrap();
        let paths = SessionPaths { dir };
        {
            let mut teams =
                CheckpointingWavWriter::create(&paths.dir.join("teams.wav"), 48_000).unwrap();
            teams.write_samples(&vec![0.25; 2400]).unwrap();
            let mut mixed = CheckpointingWavWriter::create(&paths.mixed_wav(), 48_000).unwrap();
            mixed.write_samples(&vec![0.25; 2400]).unwrap();
        }

        recover_incomplete_sessions();

        let sessions = session_store::list_finalized_sessions(10).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].meeting_wav.as_deref(), Some("teams.wav"));
        assert_eq!(sessions[0].source_app, MeetingAppId::Teams);
    }
}
