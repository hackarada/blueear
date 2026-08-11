//! Native folder picker for model-bundle import.
//!
//! SECURITY-REVIEW: the picker runs natively and hands the path straight to
//! `model_import`, so a filesystem path chosen by the user never travels
//! through the webview and the frontend cannot supply one of its own.

use std::path::PathBuf;

/// Opens a native folder picker for a model bundle and returns the chosen
/// path, or `None` if the user cancelled.
///
/// # Safety / threading
/// On macOS this must run on the main thread (`NSOpenPanel`). On Windows,
/// `rfd` is safe from the main thread as well.
pub fn pick_model_bundle_folder() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // SAFETY: caller schedules this on the main thread via
        // `run_on_main_thread`.
        unsafe { crate::transcription::native::pick_model_bundle_on_main_thread() }
    }
    #[cfg(not(target_os = "macos"))]
    {
        rfd::FileDialog::new()
            .set_title("Select a Blue Ear model bundle")
            .pick_folder()
    }
}
