mod audio;
mod commands;
mod error;
mod paths;
mod session;
mod storage;
mod transcription;
mod tray;
#[cfg(test)]
mod test_support;

use std::sync::Arc;

use tauri::Manager;

use audio::PlatformAudioEngine;
use session::SessionManager;
use transcription::events::TauriJobObserver;
use transcription::TranscriptionService;

pub const APP_BUNDLE_ID: &str = "com.blueear.app";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Repair any session left in `.inprogress-<id>` state by a crash or
    // forced quit before the UI even opens, per the plan's recovery model.
    storage::recovery::recover_incomplete_sessions();

    // A transcription that was running when the app died becomes a failed job
    // the user can retry, and a half-copied model bundle is discarded. Both are
    // cheap directory scans and neither loads a model, so a recording-only
    // user pays nothing for them.
    transcription::store::recover_interrupted_jobs();
    transcription::store::clean_model_staging_dirs();

    tauri::Builder::default()
        .setup(|app| {
            let (engine, status_rx) = PlatformAudioEngine::new();
            let manager = SessionManager::new(engine, app.handle().clone());
            manager.spawn_status_listener(status_rx);
            app.manage(manager);

            app.manage(TranscriptionService::new(
                transcription::production_registry(),
                Arc::new(TauriJobObserver::new(app.handle().clone())),
            ));

            tray::build_tray(app.handle())?;

            // The app stays running with just a menu bar presence when the
            // window is closed; only the tray's Quit item or Cmd+Q actually
            // exits, matching the Dock icon we deliberately kept.
            if let Some(window) = app.get_webview_window("main") {
                let hide_target = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = hide_target.hide();
                    }
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_readiness,
            commands::request_capture_access,
            commands::start_recording,
            commands::stop_recording,
            commands::get_session_state,
            commands::reveal_session,
            commands::list_recent_sessions,
            commands::dismiss_session,
            commands::get_session_asset_path,
            commands::get_transcription_overview,
            commands::set_transcription_preferences,
            commands::start_transcription,
            commands::retry_transcription,
            commands::cancel_transcription,
            commands::get_transcription_job,
            commands::get_transcript,
            commands::export_transcript,
            commands::import_model_bundle,
            commands::delete_model_bundle,
            commands::open_model_download_page,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app_handle, event| {
            // macOS-only: `applicationShouldHandleReopen`, fired when the
            // Dock icon is clicked while the app has no visible windows.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen {
                has_visible_windows,
                ..
            } = event
            {
                if !has_visible_windows {
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                }
            }

            #[cfg(not(target_os = "macos"))]
            {
                let _ = (app_handle, event);
            }
        });
}

/// Managed alongside [`SessionManager`] so `emit_state` can reflect the
/// current session state in the menu bar without a second source of truth.
pub(crate) struct TrayHandles {
    pub toggle_item: tauri::menu::MenuItem<tauri::Wry>,
}
