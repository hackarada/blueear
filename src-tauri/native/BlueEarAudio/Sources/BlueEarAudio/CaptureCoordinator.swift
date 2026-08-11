// CaptureCoordinator.swift
//
// Top-level owner of both native audio sources (meeting-app process tap and
// optional microphone). This is the only object BlueEarAudioFFI.swift talks
// to; it translates the narrow C ABI into calls on ProcessTap /
// MicrophoneCapture and forwards their callbacks back out through the
// closures registered by `blueear_audio_init`.

import AVFoundation
import CoreAudio
import Foundation

final class CaptureCoordinator {
    private var emitAudio: ((BlueEarAudioSource, UnsafePointer<Float>, UInt32, UInt32, Double, UInt64) -> Void)?
    private var emitStatus: ((BlueEarStatusEvent, Int32) -> Void)?

    private lazy var meetingTap = ProcessTap(
        onAudio: { [weak self] samples, frameCount, channelCount, sampleRate, hostTimeNs in
            self?.emitAudio?(.meeting, samples, frameCount, channelCount, sampleRate, hostTimeNs)
        },
        onStatus: { [weak self] event, detail in
            self?.emitStatus?(event, detail)
        }
    )

    private lazy var microphone = MicrophoneCapture(
        onAudio: { [weak self] samples, frameCount, channelCount, sampleRate, hostTimeNs in
            self?.emitAudio?(.microphone, samples, frameCount, channelCount, sampleRate, hostTimeNs)
        },
        onStatus: { [weak self] event, detail in
            self?.emitStatus?(event, detail)
        }
    )

    func configure(
        emitAudio: @escaping (BlueEarAudioSource, UnsafePointer<Float>, UInt32, UInt32, Double, UInt64) -> Void,
        emitStatus: @escaping (BlueEarStatusEvent, Int32) -> Void
    ) {
        self.emitAudio = emitAudio
        self.emitStatus = emitStatus
    }

    func startMeetingCapture(app: BlueEarMeetingApp) -> Int32 {
        meetingTap.start(app: app)
    }

    func stopMeetingCapture() {
        meetingTap.stop()
    }

    func startMicrophoneCapture() -> Int32 {
        microphone.start()
    }

    func stopMicrophoneCapture() {
        microphone.stop()
    }

    /// Builds a throwaway tap targeting the whole system (excluding no
    /// processes), starts and immediately stops it. There is no public
    /// authoritative "is kTCCServiceAudioCapture granted?" API, so actually
    /// starting I/O -- the call that triggers the permission prompt on
    /// first run -- is the only reliable way to surface (and, via whether
    /// callbacks arrive, infer the result of) that prompt ahead of a real
    /// recording.
    func probeAudioPermission() -> Int32 {
        var callbackFired = false
        let probeTap = CATapDescription(stereoGlobalTapButExcludeProcesses: [])
        probeTap.name = "Blue Ear Permission Probe"
        probeTap.isPrivate = true
        probeTap.muteBehavior = .unmuted

        var tapID = AudioObjectID(kAudioObjectUnknown)
        guard AudioHardwareCreateProcessTap(probeTap, &tapID) == noErr else {
            return 0 // unknown / could not even construct the tap object
        }
        defer { AudioHardwareDestroyProcessTap(tapID) }

        let aggregateDescription: [String: Any] = [
            kAudioAggregateDeviceNameKey: "BlueEar-Permission-Probe",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            kAudioAggregateDeviceTapListKey: [
                [kAudioSubTapUIDKey: probeTap.uuid.uuidString, kAudioSubTapDriftCompensationKey: true]
            ]
        ]
        var aggregateID = AudioObjectID(kAudioObjectUnknown)
        guard AudioHardwareCreateAggregateDevice(aggregateDescription as CFDictionary, &aggregateID) == noErr else {
            return 0
        }
        defer { AudioHardwareDestroyAggregateDevice(aggregateID) }

        var ioProcID: AudioDeviceIOProcID?
        let status = AudioDeviceCreateIOProcIDWithBlock(&ioProcID, aggregateID, nil) { _, _, _, _, _ in
            callbackFired = true
        }
        guard status == noErr, let procID = ioProcID else { return 0 }
        defer { AudioDeviceDestroyIOProcID(aggregateID, procID) }

        guard AudioDeviceStart(aggregateID, procID) == noErr else { return 0 }
        Thread.sleep(forTimeInterval: 0.3)
        AudioDeviceStop(aggregateID, procID)

        return callbackFired ? 1 : 0
    }

    func shutdown() {
        meetingTap.stop()
        microphone.stop()
        emitAudio = nil
        emitStatus = nil
    }

    /// Lightweight readiness probe: checks whether the default input device
    /// exposes a valid format without starting AVAudioEngine I/O or
    /// installing a tap (so it does not trigger the mic permission prompt).
    func microphoneInputAvailable() -> Int32 {
        let engine = AVAudioEngine()
        let format = engine.inputNode.outputFormat(forBus: 0)
        return format.sampleRate > 0 && format.channelCount > 0 ? 1 : 0
    }
}
