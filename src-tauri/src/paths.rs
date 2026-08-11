//! Platform-aware filesystem roots for recordings and app support data.
//!
//! Tests redirect the whole tree by setting `HOME` (and, on Windows hosts,
//! optionally `USERPROFILE`). Production uses the OS home / known-folder APIs.

use std::path::PathBuf;

/// User home used for Music and for test redirection.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/Music/BlueEar/Recordings` on every platform (Music under the home dir).
pub fn recordings_root() -> PathBuf {
    home_dir()
        .join("Music")
        .join("BlueEar")
        .join("Recordings")
}

/// Application support / roaming data for preferences and model bundles.
pub fn app_support_root() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        home_dir()
            .join("Library")
            .join("Application Support")
            .join(crate::APP_BUNDLE_ID)
    }
    #[cfg(target_os = "windows")]
    {
        // Prefer APPDATA in production; when HOME is set (unit tests) keep
        // everything under that home so a single env var redirects the tree.
        if std::env::var_os("HOME").is_some() {
            return home_dir()
                .join("AppData")
                .join("Roaming")
                .join(crate::APP_BUNDLE_ID);
        }
        dirs::data_dir()
            .unwrap_or_else(|| home_dir().join("AppData").join("Roaming"))
            .join(crate::APP_BUNDLE_ID)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        home_dir()
            .join(".local")
            .join("share")
            .join(crate::APP_BUNDLE_ID)
    }
}

/// Short path shown in onboarding copy.
pub fn recordings_path_display() -> String {
    #[cfg(target_os = "windows")]
    {
        "%USERPROFILE%\\Music\\BlueEar\\Recordings".to_string()
    }
    #[cfg(not(target_os = "windows"))]
    {
        "~/Music/BlueEar/Recordings".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::HOME_ENV_LOCK;

    #[test]
    fn recordings_root_respects_home_override() {
        let _guard = HOME_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("HOME", dir.path());
        assert_eq!(
            recordings_root(),
            dir.path().join("Music").join("BlueEar").join("Recordings")
        );
        std::env::remove_var("HOME");
    }
}
