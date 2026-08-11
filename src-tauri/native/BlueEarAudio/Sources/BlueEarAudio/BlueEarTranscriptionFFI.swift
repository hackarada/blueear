// BlueEarTranscriptionFFI.swift
//
// The C ABI for transcription. Deliberately separate from
// `BlueEarAudioFFI.swift`: that boundary carries fixed POD frames on a
// real-time audio thread and has to stay allocation-free, while this one
// carries variable-length structured results on a worker thread and can
// afford UTF-8 JSON. See `src-tauri/src/transcription/native.rs` for the Rust
// side and `docs/superpowers/specs/2026-08-07-local-transcription-design.md`
// for the contract.
//
// Every `char *` returned from this file is allocated with `strdup` and must
// be handed back to `blueear_transcription_string_free`. Rust does exactly
// that in `take_native_string`.
//
// SECURITY-REVIEW: this module reads audio files and CoreML models from paths
// supplied by the Rust side. Those paths are always server-resolved -- a
// session's own WAV file, or the app-owned models directory -- never a path
// that came from the webview or from a user-typed string.

import Foundation

/// C function pointer Rust registers once. Called with a job handle and a
/// 0.0-1.0 fraction, never from a real-time thread.
public typealias BlueEarTranscriptionProgressCallback = @convention(c) (
    UnsafeMutableRawPointer?,
    UInt64,
    Double
) -> Void

enum TranscriptionFFIState {
    static let coordinator = TranscriptionCoordinator()
}

@_cdecl("blueear_transcription_init")
public func blueear_transcription_init(
    progressCallback: @escaping BlueEarTranscriptionProgressCallback,
    userData: UnsafeMutableRawPointer?
) {
    TranscriptionFFIState.coordinator.configure { handle, fraction in
        progressCallback(userData, handle, fraction)
    }
}

/// Reports whether a provider could run right now, as
/// `{"ready": Bool, "reason": String?}`.
///
/// Must stay cheap and side-effect free: Rust calls it on every settings
/// refresh, and the design requires FluidAudio stay completely inert until it
/// is both selected and actually used.
@_cdecl("blueear_transcription_probe")
public func blueear_transcription_probe(
    provider: UnsafePointer<CChar>?,
    modelsRoot: UnsafePointer<CChar>?
) -> UnsafeMutablePointer<CChar>? {
    let providerId = provider.map { String(cString: $0) } ?? ""
    let root = modelsRoot.map { String(cString: $0) } ?? ""
    let probe = TranscriptionFFIState.coordinator.probe(provider: providerId, modelsRoot: root)
    return copyToC(probe.jsonString())
}

/// Transcribes one track. Blocks the calling thread for the whole inference,
/// which is why Rust calls it from a background worker and cancels through a
/// separate handle rather than by interrupting this call.
@_cdecl("blueear_transcription_run")
public func blueear_transcription_run(
    requestJSON: UnsafePointer<CChar>?,
    jobHandle: UInt64
) -> UnsafeMutablePointer<CChar>? {
    guard let requestJSON else {
        return copyToC(TranscriptionResponse.failure(.failed).jsonString())
    }
    let json = String(cString: requestJSON)
    let response = TranscriptionFFIState.coordinator.run(requestJSON: json, jobHandle: jobHandle)
    return copyToC(response.jsonString())
}

@_cdecl("blueear_transcription_cancel")
public func blueear_transcription_cancel(jobHandle: UInt64) {
    TranscriptionFFIState.coordinator.cancel(jobHandle: jobHandle)
}

@_cdecl("blueear_transcription_string_free")
public func blueear_transcription_string_free(pointer: UnsafeMutablePointer<CChar>?) {
    guard let pointer else { return }
    free(pointer)
}

/// Presents a folder picker for a model bundle and returns the chosen path, or
/// an empty string if the user cancelled. Must be called on the main thread;
/// Rust does so via Tauri's `run_on_main_thread`.
@_cdecl("blueear_transcription_pick_model_bundle")
public func blueear_transcription_pick_model_bundle() -> UnsafeMutablePointer<CChar>? {
    copyToC(ModelBundlePicker.presentOnMainThread() ?? "")
}

/// `strdup` of a Swift string, so ownership crosses to Rust cleanly.
private func copyToC(_ value: String) -> UnsafeMutablePointer<CChar>? {
    value.withCString { strdup($0) }
}
