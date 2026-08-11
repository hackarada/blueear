//! Stable, public error surface shared with the frontend.
//!
//! Per the plan's security constraints, UI-facing errors must never leak
//! stack traces or filesystem internals. Every failure inside the app gets
//! mapped to one of these codes before crossing the Tauri IPC boundary; any
//! additional detail is logged locally only (never sent to the frontend and
//! never containing PCM, paths, or participant data).

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ErrorCode {
    UnsupportedOs,
    MeetingAppNotFound,
    MeetingAppNotRunning,
    AudioPermissionDenied,
    MicPermissionDenied,
    MicUnavailable,
    SourceSilent,
    DiskFull,
    FinalizeFailed,
    SessionConflict,
    SessionNotFound,
    TrackNotFound,
    TranscriptionUnavailable,
    TranscriptionProviderNotReady,
    TranscriptionModelMissing,
    TranscriptionInvalidBundle,
    TranscriptionCancelled,
    TranscriptionInterrupted,
    TranscriptionFailed,
    TranscriptNotFound,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AppError {
    pub code: ErrorCode,
    /// Short, generic, user-safe message. Never a stack trace or raw OS
    /// error string.
    pub message: String,
}

impl AppError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn unsupported_os() -> Self {
        Self::new(
            ErrorCode::UnsupportedOs,
            "Blue Ear requires macOS 14.4 or later.",
        )
    }

    pub fn meeting_app_not_found(display_name: &str) -> Self {
        Self::new(
            ErrorCode::MeetingAppNotFound,
            format!("{display_name} was not found on this Mac."),
        )
    }

    pub fn meeting_app_not_running(display_name: &str) -> Self {
        Self::new(
            ErrorCode::MeetingAppNotRunning,
            format!("{display_name} is not currently running."),
        )
    }

    pub fn audio_permission_denied() -> Self {
        Self::new(
            ErrorCode::AudioPermissionDenied,
            "Blue Ear needs System Audio Recording access to capture meeting audio.",
        )
    }

    pub fn mic_permission_denied() -> Self {
        Self::new(
            ErrorCode::MicPermissionDenied,
            "Blue Ear needs Microphone access to record your voice.",
        )
    }

    pub fn mic_unavailable() -> Self {
        Self::new(
            ErrorCode::MicUnavailable,
            "No microphone input is available on this Mac.",
        )
    }

    pub fn source_silent() -> Self {
        Self::new(
            ErrorCode::SourceSilent,
            "Meeting audio could not be captured. Check System Audio Recording permission.",
        )
    }

    pub fn disk_full() -> Self {
        Self::new(
            ErrorCode::DiskFull,
            "Not enough free disk space to continue recording.",
        )
    }

    pub fn finalize_failed() -> Self {
        Self::new(
            ErrorCode::FinalizeFailed,
            "The recording could not be finalized.",
        )
    }

    pub fn session_conflict() -> Self {
        Self::new(
            ErrorCode::SessionConflict,
            "A recording is already in progress.",
        )
    }

    pub fn session_not_found() -> Self {
        Self::new(ErrorCode::SessionNotFound, "No matching session was found.")
    }

    pub fn track_not_found() -> Self {
        Self::new(
            ErrorCode::TrackNotFound,
            "That track wasn't recorded for this session.",
        )
    }

    /// The build or the OS cannot offer transcription at all, so there is
    /// nothing for the user to configure.
    pub fn transcription_unavailable() -> Self {
        Self::new(
            ErrorCode::TranscriptionUnavailable,
            "Transcription isn't available on this Mac.",
        )
    }

    /// A provider is selected but cannot run yet. The settings screen shows
    /// the specific `NotReadyReason` alongside this; deliberately no silent
    /// fallback to another engine.
    pub fn transcription_provider_not_ready() -> Self {
        Self::new(
            ErrorCode::TranscriptionProviderNotReady,
            "The selected transcription provider isn't ready yet.",
        )
    }

    pub fn transcription_model_missing() -> Self {
        Self::new(
            ErrorCode::TranscriptionModelMissing,
            "No transcription model is installed.",
        )
    }

    pub fn transcription_invalid_bundle() -> Self {
        Self::new(
            ErrorCode::TranscriptionInvalidBundle,
            "That model bundle could not be verified and was not installed.",
        )
    }

    pub fn transcription_cancelled() -> Self {
        Self::new(ErrorCode::TranscriptionCancelled, "Transcription cancelled.")
    }

    /// A job that was still running when the app quit. Retryable, which is why
    /// it is distinct from a genuine failure.
    pub fn transcription_interrupted() -> Self {
        Self::new(
            ErrorCode::TranscriptionInterrupted,
            "Transcription was interrupted and can be retried.",
        )
    }

    pub fn transcription_failed() -> Self {
        Self::new(
            ErrorCode::TranscriptionFailed,
            "The recording could not be transcribed.",
        )
    }

    pub fn transcript_not_found() -> Self {
        Self::new(
            ErrorCode::TranscriptNotFound,
            "This recording hasn't been transcribed yet.",
        )
    }

    pub fn internal(context: &str) -> Self {
        log::error!("internal error: {context}");
        Self::new(
            ErrorCode::Internal,
            "Something went wrong. Please try again.",
        )
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

pub type AppResult<T> = Result<T, AppError>;
