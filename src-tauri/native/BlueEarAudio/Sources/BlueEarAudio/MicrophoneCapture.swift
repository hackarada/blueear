// MicrophoneCapture.swift
//
// Optional local microphone capture via AVAudioEngine. Off by default; only
// started when the user enables the microphone toggle before recording.
// Captures the system default input device without changing it, and
// restarts itself when the input device/format changes (e.g. headphones or
// AirPods connect/disconnect mid-recording).

import AVFoundation
import CoreAudio
import Foundation

enum MicrophoneStartError: Int32 {
    case alreadyRunning = -1
    case invalidFormat = -2
    case engineStartFailed = -3
}

final class MicrophoneCapture {
    typealias AudioSink = ProcessTap.AudioSink
    typealias StatusSink = ProcessTap.StatusSink

    private let onAudio: AudioSink
    private let onStatus: StatusSink
    private var engine: AVAudioEngine?
    private var running = false
    private var scratchBuffer = [Float](repeating: 0, count: 8192 * 2)
    private let stateQueue = DispatchQueue(label: "com.blueear.audio.mic.state")
    private var configObserver: NSObjectProtocol?

    init(onAudio: @escaping AudioSink, onStatus: @escaping StatusSink) {
        self.onAudio = onAudio
        self.onStatus = onStatus
    }

    func start() -> Int32 {
        var result: Int32 = 0
        stateQueue.sync { result = self.startLocked() }
        return result
    }

    func stop() {
        stateQueue.sync { self.stopLocked(emitStoppedEvent: true) }
    }

    private func startLocked() -> Int32 {
        guard !running else { return MicrophoneStartError.alreadyRunning.rawValue }

        let engine = AVAudioEngine()
        let inputNode = engine.inputNode
        let format = inputNode.outputFormat(forBus: 0)
        guard format.sampleRate > 0, format.channelCount > 0 else {
            return MicrophoneStartError.invalidFormat.rawValue
        }
        let sampleRate = format.sampleRate
        let channelCount = format.channelCount
        let isInterleaved = format.isInterleaved

        inputNode.installTap(onBus: 0, bufferSize: 2048, format: format) { [weak self] buffer, time in
            guard let self = self, let channelData = buffer.floatChannelData else { return }
            let frameCount = buffer.frameLength
            guard frameCount > 0 else { return }
            let hostTimeNs = time.hostTime > 0 ? AudioConvertHostTimeToNanos(time.hostTime) : 0

            if isInterleaved || channelCount == 1 {
                self.onAudio(channelData[0], frameCount, channelCount, sampleRate, hostTimeNs)
            } else {
                let needed = Int(frameCount) * Int(channelCount)
                if self.scratchBuffer.count < needed {
                    self.scratchBuffer = [Float](repeating: 0, count: needed)
                }
                self.scratchBuffer.withUnsafeMutableBufferPointer { scratch in
                    for channel in 0..<Int(channelCount) {
                        let src = channelData[channel]
                        for frame in 0..<Int(frameCount) {
                            scratch[frame * Int(channelCount) + channel] = src[frame]
                        }
                    }
                }
                self.scratchBuffer.withUnsafeBufferPointer { scratch in
                    guard let base = scratch.baseAddress else { return }
                    self.onAudio(base, frameCount, channelCount, sampleRate, hostTimeNs)
                }
            }
        }

        do {
            try engine.start()
        } catch {
            inputNode.removeTap(onBus: 0)
            return MicrophoneStartError.engineStartFailed.rawValue
        }

        self.engine = engine
        running = true
        onStatus(.micStarted, 0)

        configObserver = NotificationCenter.default.addObserver(
            forName: .AVAudioEngineConfigurationChange,
            object: engine,
            queue: nil
        ) { [weak self] _ in
            self?.handleConfigurationChange()
        }

        return 0
    }

    private func handleConfigurationChange() {
        stateQueue.async { [weak self] in
            guard let self = self, self.running else { return }
            self.onStatus(.micDeviceChanged, 0)
            self.stopLocked(emitStoppedEvent: false)
            _ = self.startLocked()
        }
    }

    private func stopLocked(emitStoppedEvent: Bool) {
        guard running else { return }
        if let observer = configObserver {
            NotificationCenter.default.removeObserver(observer)
            configObserver = nil
        }
        engine?.inputNode.removeTap(onBus: 0)
        engine?.stop()
        engine = nil
        running = false
        if emitStoppedEvent {
            onStatus(.micStopped, 0)
        }
    }
}
