//! Every Tauri command Blue Ear exposes to the frontend. Each one is a thin
//! wrapper around [`SessionManager`] or [`TranscriptionService`]; no business
//! logic lives here so the IPC surface stays easy to audit against the plan's
//! Tauri API contract.
//!
//! Two rules hold across the whole surface. Filesystem paths are resolved
//! server-side from a session ID and are never accepted from the frontend, and
//! errors cross as [`AppError`] codes with generic messages, so native failure
//! detail never reaches the webview.

use std::sync::Arc;

use tauri::{AppHandle, State};

use crate::audio::MeetingAppId;
use crate::error::{AppError, AppResult};
use crate::session::{PermissionState, Readiness, SessionManager, SessionState};
use crate::storage::session_store::SessionMetadata;
use crate::transcription::model_import;
use crate::transcription::service::TranscriptionOverview;
use crate::transcription::store::Preferences;
use crate::transcription::types::{Job, Transcript};
use crate::transcription::TranscriptionService;

#[tauri::command]
pub fn get_readiness(manager: State<'_, Arc<SessionManager>>) -> Readiness {
    manager.get_readiness()
}

#[tauri::command]
pub fn request_capture_access(manager: State<'_, Arc<SessionManager>>) -> PermissionState {
    manager.request_capture_access()
}

#[tauri::command]
pub fn start_recording(
    manager: State<'_, Arc<SessionManager>>,
    source_app: MeetingAppId,
    include_microphone: bool,
) -> AppResult<String> {
    manager.start_recording(source_app, include_microphone)
}

#[tauri::command]
pub fn stop_recording(
    manager: State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<SessionMetadata> {
    manager.stop_recording(&session_id)
}

#[tauri::command]
pub fn get_session_state(manager: State<'_, Arc<SessionManager>>) -> SessionState {
    manager.get_state()
}

#[tauri::command]
pub fn reveal_session(
    manager: State<'_, Arc<SessionManager>>,
    session_id: String,
) -> AppResult<()> {
    manager.reveal_session(&session_id)
}

#[tauri::command]
pub fn get_session_asset_path(
    manager: State<'_, Arc<SessionManager>>,
    session_id: String,
    track: String,
) -> AppResult<String> {
    manager.get_session_asset_path(&session_id, &track)
}

#[tauri::command]
pub fn list_recent_sessions(
    manager: State<'_, Arc<SessionManager>>,
    limit: usize,
) -> AppResult<Vec<SessionMetadata>> {
    manager.list_recent_sessions(limit)
}

#[tauri::command]
pub fn dismiss_session(manager: State<'_, Arc<SessionManager>>) {
    manager.dismiss();
}

// MARK: - Transcription

/// Every provider with its current readiness, the user's selection, and the
/// installed bundles, so settings can show the trade-offs side by side rather
/// than hiding the options that still need work.
#[tauri::command]
pub fn get_transcription_overview(
    service: State<'_, Arc<TranscriptionService>>,
) -> TranscriptionOverview {
    service.overview()
}

#[tauri::command]
pub fn set_transcription_preferences(
    service: State<'_, Arc<TranscriptionService>>,
    preferences: Preferences,
) -> AppResult<TranscriptionOverview> {
    service.set_preferences(&preferences)?;
    Ok(service.overview())
}

#[tauri::command]
pub fn start_transcription(
    service: State<'_, Arc<TranscriptionService>>,
    session_id: String,
) -> AppResult<Job> {
    service.start(&session_id)
}

#[tauri::command]
pub fn retry_transcription(
    service: State<'_, Arc<TranscriptionService>>,
    session_id: String,
) -> AppResult<Job> {
    service.retry(&session_id)
}

#[tauri::command]
pub fn cancel_transcription(
    service: State<'_, Arc<TranscriptionService>>,
    session_id: String,
) -> AppResult<()> {
    service.cancel(&session_id)
}

#[tauri::command]
pub fn get_transcription_job(
    service: State<'_, Arc<TranscriptionService>>,
    session_id: String,
) -> AppResult<Option<Job>> {
    service.job(&session_id)
}

#[tauri::command]
pub fn get_transcript(
    service: State<'_, Arc<TranscriptionService>>,
    session_id: String,
) -> AppResult<Transcript> {
    service.transcript(&session_id)
}

/// Writes a `.txt` or `.vtt` beside the recording and returns its path so the
/// frontend can reveal it. The path is derived from the session ID, never
/// supplied by the caller.
#[tauri::command]
pub fn export_transcript(
    service: State<'_, Arc<TranscriptionService>>,
    session_id: String,
    format: String,
) -> AppResult<String> {
    service.export(&session_id, &format)
}

#[tauri::command]
pub fn delete_model_bundle(
    service: State<'_, Arc<TranscriptionService>>,
    bundle_id: String,
) -> AppResult<TranscriptionOverview> {
    model_import::delete_bundle(&bundle_id)?;
    Ok(service.overview())
}

/// Presents the native folder picker and imports whatever the user chose.
/// Returns the refreshed settings snapshot, or `None` if the user dismissed
/// the panel, which is not an error.
///
/// SECURITY-REVIEW: the only entry point for user-supplied model data. The
/// path comes from `NSOpenPanel` rather than from the frontend, and everything
/// after it runs through `model_import`'s validation before a byte is loaded.
///
/// Declared `async` on purpose: Tauri runs synchronous commands on the main
/// thread, and this one blocks on the main thread presenting a modal panel, so
/// running it there would deadlock.
#[tauri::command]
pub async fn import_model_bundle(
    app_handle: AppHandle,
    service: State<'_, Arc<TranscriptionService>>,
) -> AppResult<Option<TranscriptionOverview>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    app_handle
        .run_on_main_thread(move || {
            let picked = crate::transcription::picker::pick_model_bundle_folder();
            let _ = sender.send(picked);
        })
        .map_err(|_| AppError::internal("run_on_main_thread"))?;

    let Some(source) = receiver
        .recv()
        .map_err(|_| AppError::internal("model bundle picker channel"))?
    else {
        return Ok(None);
    };

    model_import::import_bundle(&source)?;
    Ok(Some(service.overview()))
}

/// Which Hugging Face model page the settings screen may ask to open.
/// The frontend only sends this enum; the URL itself never crosses IPC.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ModelDownloadPage {
    Asr,
    Diarization,
}

/// Opens an allowlisted model page in the system browser.
///
/// SECURITY-REVIEW: the URL is chosen entirely server-side from a closed enum.
/// The frontend cannot supply an arbitrary URL. Blue Ear itself performs no
/// network I/O -- the OS opener hands the URL to the user's default browser.
#[tauri::command]
pub fn open_model_download_page(page: ModelDownloadPage) -> AppResult<()> {
    let url = match page {
        ModelDownloadPage::Asr => {
            "https://huggingface.co/FluidInference/parakeet-tdt-0.6b-v3-coreml"
        }
        ModelDownloadPage::Diarization => {
            "https://huggingface.co/FluidInference/speaker-diarization-coreml"
        }
    };

    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|_| AppError::internal("open model download page"))?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()
            .map_err(|_| AppError::internal("open model download page"))?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = url;
        return Err(AppError::internal("open model download page unsupported"));
    }
    Ok(())
}
