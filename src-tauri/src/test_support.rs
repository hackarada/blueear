//! Shared helpers for unit tests only (`#[cfg(test)]`).

use std::sync::Mutex;

/// `storage::session_store::recordings_root()` reads the process-global
/// `HOME` env var, and `cargo test` runs tests across the whole crate on
/// multiple threads within one process by default. Any test that calls
/// `std::env::set_var("HOME", ..)` must hold this lock for the duration of
/// the env var override, or it can race with an unrelated test in another
/// module doing the same thing and observe the wrong tempdir mid-test.
pub static HOME_ENV_LOCK: Mutex<()> = Mutex::new(());
