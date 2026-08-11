//! Menu bar (tray) icon: a thin convenience layered on top of
//! [`SessionManager`], which stays the single source of truth for recording
//! state. The tray never touches audio/session logic directly -- it only
//! calls the same public `SessionManager` methods the Tauri commands call,
//! and reads state the same way `get_session_state` does.
//!
//! On macOS the tray uses a dedicated monochrome template icon
//! (`icons/tray-icon.png`) so it adapts to light and dark menu bars.

use std::sync::Arc;

use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager};

use crate::session::{SessionManager, SessionState};
use crate::TrayHandles;

const TOGGLE_ID: &str = "toggle_recording";
const SHOW_WINDOW_ID: &str = "show_window";
const SHOW_RECORDINGS_ID: &str = "show_recordings";

/// Builds the menu bar icon and its menu, and stashes the toggle item's
/// handle (via [`TrayHandles`]) so `SessionManager::emit_state` can keep its
/// label in sync with the real session state.
pub fn build_tray(app: &AppHandle) -> tauri::Result<()> {
    let toggle_item = MenuItem::with_id(app, TOGGLE_ID, "Start Recording", true, None::<&str>)?;
    let show_item = MenuItem::with_id(app, SHOW_WINDOW_ID, "Show Blue Ear", true, None::<&str>)?;
    let recordings_item =
        MenuItem::with_id(app, SHOW_RECORDINGS_ID, "Recent Recordings", true, None::<&str>)?;

    let menu = Menu::with_items(
        app,
        &[
            &toggle_item,
            &PredefinedMenuItem::separator(app)?,
            &show_item,
            &recordings_item,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let icon = load_tray_icon()?;

    let mut builder = TrayIconBuilder::new()
        .icon(icon)
        .tooltip("Blue Ear")
        .menu(&menu)
        .show_menu_on_left_click(true);

    #[cfg(target_os = "macos")]
    {
        builder = builder.icon_as_template(true);
    }

    builder
        .on_menu_event(move |app_handle, event| match event.id().0.as_str() {
            TOGGLE_ID => handle_toggle_recording(app_handle),
            SHOW_WINDOW_ID => show_main_window(app_handle),
            SHOW_RECORDINGS_ID => show_recordings(app_handle),
            _ => {}
        })
        .build(app)?;

    app.manage(TrayHandles { toggle_item });

    Ok(())
}

fn load_tray_icon() -> tauri::Result<Image<'static>> {
    Image::from_bytes(include_bytes!("../icons/tray-icon.png"))
}

fn handle_toggle_recording(app_handle: &AppHandle) {
    let Some(manager) = app_handle.try_state::<Arc<SessionManager>>() else {
        return;
    };
    match manager.get_state() {
        SessionState::Idle | SessionState::Completed { .. } | SessionState::Failed { .. } => {
            let _ = manager.start_recording(
                manager.preferred_source_app(),
                manager.preferred_include_microphone(),
            );
        }
        SessionState::Recording { session_id, .. } | SessionState::Recovering { session_id, .. } => {
            let _ = manager.stop_recording(&session_id);
        }
        // Transient states (Preparing/Stopping/Finalizing): the item is
        // disabled while these are active, see `SessionManager::emit_state`.
        _ => {}
    }
}

fn show_main_window(app_handle: &AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn show_recordings(app_handle: &AppHandle) {
    show_main_window(app_handle);
    let _ = app_handle.emit("navigate", serde_json::json!({ "screen": "recordings" }));
}
