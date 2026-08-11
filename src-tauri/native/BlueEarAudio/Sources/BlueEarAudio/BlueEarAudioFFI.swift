// BlueEarAudioFFI.swift
//
// The entire C ABI surface between Rust and this Swift package. Rust only
// ever calls the `blueear_*` functions declared here; every other Swift file
// in this package is an implementation detail reached through
// `CaptureCoordinator`.
//
// Audio data crosses the boundary as raw interleaved Float32 PCM plus
// sample rate / channel count / frame count / host timestamp -- never as an
// Objective-C or Swift object -- so Rust's `extern "C"` declarations
// (see `src-tauri/src/audio/ffi.rs`) can stay simple and stable.
//
// Meeting-app identity crosses as `BlueEarMeetingApp` Int32 only — never as
// a free-form path or bundle id string.
//
// SECURITY-REVIEW: this module is the boundary where native macOS device
// access (Core Audio process taps + microphone) is exposed to the rest of
// the app. It performs no filesystem or network I/O itself.

import Foundation

/// Matches `blueear::audio::ffi::AudioSource` on the Rust side.
@objc public enum BlueEarAudioSource: Int32 {
    case meeting = 0
    case microphone = 1
}

/// Matches `blueear::audio::ffi::StatusEvent` on the Rust side.
/// Numeric discriminants stay stable across the Teams → meeting rename.
@objc public enum BlueEarStatusEvent: Int32 {
    case sourceTapStarted = 0
    case sourceTapStopped = 1
    case sourceSilentWarning = 2
    case sourceRestored = 3
    case sourceProcessTreeChanged = 4
    case sourceAppNotFound = 5
    case micStarted = 6
    case micStopped = 7
    case micDeviceChanged = 8
    case audioPermissionDenied = 9
    case audioPermissionGranted = 10
    case genericError = 11
}

/// C function pointer type Rust registers once via `blueear_audio_init` and
/// this package invokes from real-time audio callback threads whenever a
/// buffer of PCM is ready. Must stay allocation-free and non-blocking on the
/// Swift side; Rust's trampoline pushes straight into a bounded ring buffer.
public typealias BlueEarAudioCallback = @convention(c) (
    UnsafeMutableRawPointer?,
    Int32,
    UnsafePointer<Float>?,
    UInt32,
    UInt32,
    Double,
    UInt64
) -> Void

/// C function pointer type for low-frequency lifecycle/status events (never
/// called from the real-time audio thread).
public typealias BlueEarStatusCallback = @convention(c) (
    UnsafeMutableRawPointer?,
    Int32,
    Int32
) -> Void

enum FFIState {
    static var audioCallback: BlueEarAudioCallback?
    static var statusCallback: BlueEarStatusCallback?
    static var userData: UnsafeMutableRawPointer?
    static let coordinator = CaptureCoordinator()
}

@_cdecl("blueear_audio_init")
public func blueear_audio_init(
    audioCallback: @escaping BlueEarAudioCallback,
    statusCallback: @escaping BlueEarStatusCallback,
    userData: UnsafeMutableRawPointer?
) {
    FFIState.audioCallback = audioCallback
    FFIState.statusCallback = statusCallback
    FFIState.userData = userData
    FFIState.coordinator.configure(
        emitAudio: { source, samples, frameCount, channelCount, sampleRate, hostTimeNs in
            FFIState.audioCallback?(FFIState.userData, source.rawValue, samples, frameCount, channelCount, sampleRate, hostTimeNs)
        },
        emitStatus: { event, detail in
            FFIState.statusCallback?(FFIState.userData, event.rawValue, detail)
        }
    )
}

@_cdecl("blueear_is_meeting_app_running")
public func blueear_is_meeting_app_running(_ appRaw: Int32) -> Int32 {
    guard let app = MeetingAppCatalog.fromRaw(appRaw) else { return 0 }
    return MeetingAppResolver.isRunning(app) ? 1 : 0
}

@_cdecl("blueear_is_meeting_app_installed")
public func blueear_is_meeting_app_installed(_ appRaw: Int32) -> Int32 {
    guard let app = MeetingAppCatalog.fromRaw(appRaw) else { return 0 }
    return MeetingAppResolver.isInstalled(app) ? 1 : 0
}

@_cdecl("blueear_macos_version_supported")
public func blueear_macos_version_supported() -> Int32 {
    let v = ProcessInfo.processInfo.operatingSystemVersion
    if v.majorVersion > 14 { return 1 }
    if v.majorVersion == 14 && v.minorVersion >= 4 { return 1 }
    return 0
}

/// Runs a short, throwaway tap start/stop cycle purely to surface (and
/// subsequently be able to infer the result of) the macOS system-audio TCC
/// prompt. There is no public authoritative "is this authorized?" API for
/// process taps, so this probe plus the real capture's own callback health
/// is how Blue Ear infers permission state end to end.
@_cdecl("blueear_probe_audio_permission")
public func blueear_probe_audio_permission() -> Int32 {
    FFIState.coordinator.probeAudioPermission()
}

@_cdecl("blueear_microphone_input_available")
public func blueear_microphone_input_available() -> Int32 {
    FFIState.coordinator.microphoneInputAvailable()
}

/// Starts capturing the process tree for the given `BlueEarMeetingApp`.
/// Returns 0 on success or a negative `ProcessTapStartError` code.
@_cdecl("blueear_start_meeting_capture")
public func blueear_start_meeting_capture(_ appRaw: Int32) -> Int32 {
    guard let app = MeetingAppCatalog.fromRaw(appRaw) else {
        return ProcessTapStartError.invalidApp.rawValue
    }
    return FFIState.coordinator.startMeetingCapture(app: app)
}

@_cdecl("blueear_stop_meeting_capture")
public func blueear_stop_meeting_capture() {
    FFIState.coordinator.stopMeetingCapture()
}

@_cdecl("blueear_start_microphone_capture")
public func blueear_start_microphone_capture() -> Int32 {
    FFIState.coordinator.startMicrophoneCapture()
}

@_cdecl("blueear_stop_microphone_capture")
public func blueear_stop_microphone_capture() {
    FFIState.coordinator.stopMicrophoneCapture()
}

@_cdecl("blueear_shutdown")
public func blueear_shutdown() {
    FFIState.coordinator.shutdown()
    FFIState.audioCallback = nil
    FFIState.statusCallback = nil
    FFIState.userData = nil
}
