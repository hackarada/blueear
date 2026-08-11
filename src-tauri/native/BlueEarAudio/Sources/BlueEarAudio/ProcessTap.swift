// ProcessTap.swift
//
// Owns the Core Audio process-tap lifecycle for a chosen meeting app:
// resolve that app's process tree -> CATapDescription -> private tap-only
// aggregate device -> IOProc -> raw PCM delivered to Rust via a closure.
//
// Validated end to end against live Teams and Zoom installations in
// spike/capture-spike before this production version was written.
//
// SECURITY-REVIEW: creates a Core Audio process tap targeting another
// application's audio output. Scoped exclusively to allowlisted MeetingApp
// bundle path fragments; never targets arbitrary processes.

import AVFoundation
import CoreAudio
import Foundation

enum ProcessTapStartError: Int32 {
    case alreadyRunning = -1
    case sourceAppNotRunning = -2
    case noAudioOwningProcesses = -3
    case tapCreationFailed = -4
    case aggregateCreationFailed = -5
    case formatUnavailable = -6
    case ioProcCreationFailed = -7
    case startFailed = -8
    case invalidApp = -9
}

/// Accumulates a running sum-of-squares so a low-priority timer can compute
/// RMS without doing any work on the real-time audio thread beyond a couple
/// of atomic-ish additions under a cheap non-contended lock.
private final class RunningLevel {
    private var sumSquares: Double = 0
    private var sampleCount: Int = 0
    private let lock = NSLock()

    func accumulate(_ pointer: UnsafePointer<Float>, count: Int) {
        var local: Double = 0
        for i in 0..<count {
            let s = Double(pointer[i])
            local += s * s
        }
        lock.lock()
        sumSquares += local
        sampleCount += count
        lock.unlock()
    }

    /// Returns RMS over everything accumulated since the last call, then
    /// resets the window.
    func drainRMS() -> Float? {
        lock.lock()
        defer { sumSquares = 0; sampleCount = 0; lock.unlock() }
        guard sampleCount > 0 else { return nil }
        return Float(sqrt(sumSquares / Double(sampleCount)))
    }
}

final class ProcessTap {
    typealias AudioSink = (_ samples: UnsafePointer<Float>, _ frameCount: UInt32, _ channelCount: UInt32, _ sampleRate: Double, _ hostTimeNs: UInt64) -> Void
    typealias StatusSink = (_ event: BlueEarStatusEvent, _ detail: Int32) -> Void

    private let onAudio: AudioSink
    private let onStatus: StatusSink

    private var app: BlueEarMeetingApp?
    private var tapID = AudioObjectID(kAudioObjectUnknown)
    private var aggregateID = AudioObjectID(kAudioObjectUnknown)
    private var ioProcID: AudioDeviceIOProcID?
    private var running = false
    private var lastAudioObjectIDs: Set<AudioObjectID> = []
    private var scratchBuffer = [Float](repeating: 0, count: 8192 * 2)

    private let level = RunningLevel()
    private var monitorTimer: DispatchSourceTimer?
    private var isCurrentlySilent = false
    private let silenceRMSThreshold: Float = 0.0008
    private let stateQueue = DispatchQueue(label: "com.blueear.audio.processtap.state")

    init(onAudio: @escaping AudioSink, onStatus: @escaping StatusSink) {
        self.onAudio = onAudio
        self.onStatus = onStatus
    }

    func start(app: BlueEarMeetingApp) -> Int32 {
        var result: Int32 = 0
        stateQueue.sync {
            result = self.startLocked(app: app)
        }
        return result
    }

    func stop() {
        stateQueue.sync {
            self.stopLocked(emitStoppedEvent: true)
        }
    }

    // MARK: - Core lifecycle (must run on stateQueue)

    private func startLocked(app: BlueEarMeetingApp) -> Int32 {
        guard !running else { return ProcessTapStartError.alreadyRunning.rawValue }
        guard MeetingAppCatalog.descriptor(for: app) != nil else {
            return ProcessTapStartError.invalidApp.rawValue
        }
        self.app = app

        let processes = MeetingAppResolver.discoverProcesses(for: app)
        guard !processes.isEmpty else {
            onStatus(.sourceAppNotFound, app.rawValue)
            return ProcessTapStartError.sourceAppNotRunning.rawValue
        }

        let audioObjectIDs: [AudioObjectID] = processes.compactMap { translatePIDToAudioObject($0.pid) }
        guard !audioObjectIDs.isEmpty else {
            return ProcessTapStartError.noAudioOwningProcesses.rawValue
        }
        lastAudioObjectIDs = Set(audioObjectIDs)

        let label = MeetingAppCatalog.descriptor(for: app)?.wireId ?? "meeting"
        let tapDescription = CATapDescription(stereoMixdownOfProcesses: audioObjectIDs)
        tapDescription.name = "Blue Ear \(label) Capture"
        tapDescription.isPrivate = true
        tapDescription.muteBehavior = .unmuted

        var localTapID = AudioObjectID(kAudioObjectUnknown)
        guard AudioHardwareCreateProcessTap(tapDescription, &localTapID) == noErr else {
            return ProcessTapStartError.tapCreationFailed.rawValue
        }
        tapID = localTapID

        let aggregateDescription: [String: Any] = [
            kAudioAggregateDeviceNameKey: "BlueEar-\(label)-Aggregate",
            kAudioAggregateDeviceUIDKey: UUID().uuidString,
            kAudioAggregateDeviceIsPrivateKey: true,
            kAudioAggregateDeviceIsStackedKey: false,
            // Tap-only: no physical sub-device, so output-device switches
            // (e.g. AirPods entering/leaving HFP) cannot desync or silently
            // stall this aggregate's sample rate.
            kAudioAggregateDeviceTapListKey: [
                [
                    kAudioSubTapUIDKey: tapDescription.uuid.uuidString,
                    kAudioSubTapDriftCompensationKey: true
                ]
            ]
        ]

        var localAggregateID = AudioObjectID(kAudioObjectUnknown)
        guard AudioHardwareCreateAggregateDevice(aggregateDescription as CFDictionary, &localAggregateID) == noErr else {
            AudioHardwareDestroyProcessTap(tapID)
            tapID = AudioObjectID(kAudioObjectUnknown)
            return ProcessTapStartError.aggregateCreationFailed.rawValue
        }
        aggregateID = localAggregateID

        var formatAddress = AudioObjectPropertyAddress(
            mSelector: kAudioTapPropertyFormat,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var asbd = AudioStreamBasicDescription()
        var asbdSize = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        guard AudioObjectGetPropertyData(tapID, &formatAddress, 0, nil, &asbdSize, &asbd) == noErr,
              asbd.mChannelsPerFrame > 0, asbd.mSampleRate > 0 else {
            teardownLocked()
            return ProcessTapStartError.formatUnavailable.rawValue
        }
        let sampleRate = asbd.mSampleRate
        let channelCount = asbd.mChannelsPerFrame
        let isInterleaved = (asbd.mFormatFlags & kAudioFormatFlagIsNonInterleaved) == 0

        var localIOProcID: AudioDeviceIOProcID?
        let ioStatus = AudioDeviceCreateIOProcIDWithBlock(&localIOProcID, aggregateID, nil) { [weak self] _, inInputData, inInputTime, _, _ in
            guard let self = self else { return }
            let hostTimeNs = AudioConvertHostTimeToNanos(inInputTime.pointee.mHostTime)
            self.handleInput(inInputData, channelCount: channelCount, sampleRate: sampleRate, isInterleaved: isInterleaved, hostTimeNs: hostTimeNs)
        }
        guard ioStatus == noErr, let procID = localIOProcID else {
            teardownLocked()
            return ProcessTapStartError.ioProcCreationFailed.rawValue
        }
        ioProcID = procID

        guard AudioDeviceStart(aggregateID, procID) == noErr else {
            teardownLocked()
            return ProcessTapStartError.startFailed.rawValue
        }

        running = true
        isCurrentlySilent = false
        onStatus(.sourceTapStarted, Int32(audioObjectIDs.count))
        startMonitors()
        return 0
    }

    private func stopLocked(emitStoppedEvent: Bool) {
        guard running else { return }
        stopMonitors()
        teardownLocked()
        running = false
        if emitStoppedEvent {
            onStatus(.sourceTapStopped, 0)
        }
    }

    private func teardownLocked() {
        if let procID = ioProcID {
            AudioDeviceStop(aggregateID, procID)
            AudioDeviceDestroyIOProcID(aggregateID, procID)
            ioProcID = nil
        }
        if aggregateID != kAudioObjectUnknown {
            AudioHardwareDestroyAggregateDevice(aggregateID)
            aggregateID = AudioObjectID(kAudioObjectUnknown)
        }
        if tapID != kAudioObjectUnknown {
            AudioHardwareDestroyProcessTap(tapID)
            tapID = AudioObjectID(kAudioObjectUnknown)
        }
    }

    // MARK: - Real-time callback (must stay allocation-free)

    private func handleInput(_ inInputData: UnsafePointer<AudioBufferList>, channelCount: UInt32, sampleRate: Double, isInterleaved: Bool, hostTimeNs: UInt64) {
        let bufferList = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: inInputData))

        if isInterleaved || bufferList.count == 1 {
            guard let audioBuffer = bufferList.first, let mData = audioBuffer.mData else { return }
            let frameCount = audioBuffer.mDataByteSize / UInt32(MemoryLayout<Float>.size) / max(channelCount, 1)
            guard frameCount > 0 else { return }
            let floatPointer = mData.assumingMemoryBound(to: Float.self)
            onAudio(floatPointer, frameCount, channelCount, sampleRate, hostTimeNs)
            level.accumulate(floatPointer, count: Int(frameCount * channelCount))
        } else {
            // Planar fallback: interleave into a preallocated scratch buffer
            // (resized only if genuinely necessary, never inside the steady
            // state path once warmed up).
            guard let firstBuffer = bufferList.first, firstBuffer.mDataByteSize > 0 else { return }
            let frameCount = Int(firstBuffer.mDataByteSize) / MemoryLayout<Float>.size
            let neededCapacity = frameCount * Int(channelCount)
            if scratchBuffer.count < neededCapacity {
                scratchBuffer = [Float](repeating: 0, count: neededCapacity)
            }
            scratchBuffer.withUnsafeMutableBufferPointer { scratch in
                for channel in 0..<min(bufferList.count, Int(channelCount)) {
                    guard let src = bufferList[channel].mData?.assumingMemoryBound(to: Float.self) else { continue }
                    for frame in 0..<frameCount {
                        scratch[frame * Int(channelCount) + channel] = src[frame]
                    }
                }
            }
            scratchBuffer.withUnsafeBufferPointer { scratch in
                guard let base = scratch.baseAddress else { return }
                onAudio(base, UInt32(frameCount), channelCount, sampleRate, hostTimeNs)
                level.accumulate(base, count: neededCapacity)
            }
        }
    }

    // MARK: - Background monitors (never touch the real-time thread)

    private func startMonitors() {
        let timer = DispatchSource.makeTimerSource(queue: DispatchQueue(label: "com.blueear.audio.processtap.monitor"))
        timer.schedule(deadline: .now() + 2, repeating: 2)
        timer.setEventHandler { [weak self] in
            self?.runMonitorTick()
        }
        timer.resume()
        monitorTimer = timer
    }

    private func stopMonitors() {
        monitorTimer?.cancel()
        monitorTimer = nil
    }

    private func runMonitorTick() {
        // Silence detection.
        if let rms = level.drainRMS() {
            let silentNow = rms < silenceRMSThreshold
            if silentNow != isCurrentlySilent {
                isCurrentlySilent = silentNow
                onStatus(silentNow ? .sourceSilentWarning : .sourceRestored, 0)
            }
        }

        // Process-tree drift detection: if the meeting app spawned/retired a
        // helper process that owns audio, rebuild the tap so the new
        // process is included (or a dead one stops being referenced).
        guard let app = app else { return }
        let currentProcesses = MeetingAppResolver.discoverProcesses(for: app)
        if currentProcesses.isEmpty {
            onStatus(.sourceAppNotFound, app.rawValue)
            return
        }
        let currentAudioObjectIDs = Set(currentProcesses.compactMap { translatePIDToAudioObject($0.pid) })
        guard !currentAudioObjectIDs.isEmpty, currentAudioObjectIDs != lastAudioObjectIDs else { return }

        stateQueue.async { [weak self] in
            guard let self = self, self.running, let app = self.app else { return }
            self.onStatus(.sourceProcessTreeChanged, 0)
            self.stopLocked(emitStoppedEvent: false)
            _ = self.startLocked(app: app)
        }
    }
}

func translatePIDToAudioObject(_ pid: pid_t) -> AudioObjectID? {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyTranslatePIDToProcessObject,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var pidVar = pid
    var objectID = AudioObjectID(kAudioObjectUnknown)
    var dataSize = UInt32(MemoryLayout<AudioObjectID>.size)
    let status = AudioObjectGetPropertyData(
        AudioObjectID(kAudioObjectSystemObject),
        &address,
        UInt32(MemoryLayout<pid_t>.size),
        &pidVar,
        &dataSize,
        &objectID
    )
    guard status == noErr, objectID != kAudioObjectUnknown else { return nil }
    return objectID
}
