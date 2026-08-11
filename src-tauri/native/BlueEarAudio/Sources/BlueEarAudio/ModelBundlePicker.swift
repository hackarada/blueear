// ModelBundlePicker.swift
//
// A native folder picker for model bundles.
//
// Blue Ear could have used a Tauri dialog plugin for this, but that would mean
// granting the webview a plugin permission and letting a filesystem path
// round-trip through JavaScript. Doing it natively keeps
// `capabilities/default.json` at `core:default` and keeps the invariant that
// no path chosen by a human ever crosses the IPC boundary: the picker's result
// goes straight from AppKit to Rust's validator.
//
// SECURITY-REVIEW: the returned path is user-controlled and is treated as
// untrusted by `src-tauri/src/transcription/model_import.rs`, which validates
// the whole bundle and copies it into an app-owned directory before anything
// is ever loaded from it.

import AppKit
import Foundation

enum ModelBundlePicker {

    /// Presents the panel and returns the selected directory path, or `nil` if
    /// the user cancelled.
    ///
    /// Must be called on the main thread. Rust guarantees that by invoking the
    /// FFI entry point through Tauri's `run_on_main_thread`; the precondition
    /// below turns a violation into an obvious crash rather than AppKit
    /// misbehaving in a way that is hard to trace.
    static func presentOnMainThread() -> String? {
        precondition(Thread.isMainThread, "the model bundle picker must run on the main thread")

        let panel = NSOpenPanel()
        panel.title = "Choose a Blue Ear model bundle"
        panel.message =
            "Select the folder containing the model bundle's manifest.json. Blue Ear verifies it and copies the models into its own storage."
        panel.prompt = "Import"
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.resolvesAliases = false
        panel.showsHiddenFiles = false

        guard panel.runModal() == .OK, let url = panel.url else {
            return nil
        }
        return url.path
    }
}
