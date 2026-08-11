// TranscriptionCoordinator.swift
//
// Routes a transcription request to the right adapter, owns the cancellation
// registry, and bridges the adapters' async APIs to the synchronous C ABI.
//
// Everything the adapters produce is flattened into words plus optional
// speaker spans here. Deciding what those spans mean -- which word belongs to
// which speaker, what a speaker is called, how tracks interleave -- is Rust's
// job, in `src-tauri/src/transcription/merge.rs`, where it can be tested
// without a model.

import Foundation

// MARK: - Wire types

/// Mirrors `NativeRequest` in `src-tauri/src/transcription/native.rs`.
struct TranscriptionRequest: Decodable {
    let provider: String
    let track: String
    let audioPath: String
    let modelsRoot: String
    let diarize: Bool
    let language: String?
}

struct TranscribedWord: Encodable {
    let text: String
    let startSeconds: Double
    let endSeconds: Double
    let confidence: Float?
}

struct TranscribedSpeakerSpan: Encodable {
    let speakerKey: String
    let startSeconds: Double
    let endSeconds: Double
}

/// Stable error codes. Rust maps these onto its own `ErrorCode`; anything it
/// does not recognize becomes a generic failure, so adding a case here can
/// never introduce new user-visible error text by itself.
enum TranscriptionErrorCode: String {
    case cancelled
    case modelsMissing = "models_missing"
    case providerNotReady = "provider_not_ready"
    case unsupportedOS = "unsupported_os"
    case notBuilt = "not_built"
    case audioUnreadable = "audio_unreadable"
    case failed
}

struct TranscriptionResponse: Encodable {
    let ok: Bool
    let errorCode: String?
    let words: [TranscribedWord]?
    let speakerSpans: [TranscribedSpeakerSpan]?
    let modelId: String?
    let language: String?

    static func success(
        words: [TranscribedWord],
        speakerSpans: [TranscribedSpeakerSpan],
        modelId: String?,
        language: String?
    ) -> TranscriptionResponse {
        TranscriptionResponse(
            ok: true, errorCode: nil, words: words, speakerSpans: speakerSpans,
            modelId: modelId, language: language)
    }

    static func failure(_ code: TranscriptionErrorCode) -> TranscriptionResponse {
        TranscriptionResponse(
            ok: false, errorCode: code.rawValue, words: nil, speakerSpans: nil,
            modelId: nil, language: nil)
    }

    func jsonString() -> String {
        guard let data = try? JSONEncoder().encode(self),
            let json = String(data: data, encoding: .utf8)
        else {
            // Hand-written so a failure to encode a failure cannot recurse.
            return #"{"ok":false,"errorCode":"failed"}"#
        }
        return json
    }
}

/// Mirrors `NativeProbe` on the Rust side. `reason` uses the camelCase
/// spellings of `NotReadyReason`.
struct TranscriptionProbe: Encodable {
    let ready: Bool
    let reason: String?

    static let ok = TranscriptionProbe(ready: true, reason: nil)

    static func notReady(_ reason: String) -> TranscriptionProbe {
        TranscriptionProbe(ready: false, reason: reason)
    }

    func jsonString() -> String {
        guard let data = try? JSONEncoder().encode(self),
            let json = String(data: data, encoding: .utf8)
        else {
            return #"{"ready":false,"reason":"probeFailed"}"#
        }
        return json
    }
}

/// What an adapter returns before it is turned into a response.
struct AdapterResult {
    let words: [TranscribedWord]
    let speakerSpans: [TranscribedSpeakerSpan]
    let modelId: String?
    let language: String?
}

/// Thrown by adapters so the coordinator can map them to stable codes without
/// letting a `localizedDescription` leak across the boundary.
struct AdapterError: Error {
    let code: TranscriptionErrorCode
}

// MARK: - Coordinator

final class TranscriptionCoordinator: @unchecked Sendable {
    private let lock = NSLock()
    private var cancelledJobs: Set<UInt64> = []
    private var emitProgress: ((UInt64, Double) -> Void)?

    func configure(emitProgress: @escaping (UInt64, Double) -> Void) {
        lock.lock()
        defer { lock.unlock() }
        self.emitProgress = emitProgress
    }

    func cancel(jobHandle: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        cancelledJobs.insert(jobHandle)
    }

    func isCancelled(_ jobHandle: UInt64) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return cancelledJobs.contains(jobHandle)
    }

    private func finish(jobHandle: UInt64) {
        lock.lock()
        defer { lock.unlock() }
        cancelledJobs.remove(jobHandle)
    }

    private func report(jobHandle: UInt64, fraction: Double) {
        lock.lock()
        let callback = emitProgress
        lock.unlock()
        callback?(jobHandle, fraction)
    }

    // MARK: Probing

    func probe(provider: String, modelsRoot: String) -> TranscriptionProbe {
        switch provider {
        case "apple_speech":
            return AppleSpeechAdapter.probe()
        case "fluidaudio":
            return FluidAudioAdapter.probe(modelsRoot: URL(fileURLWithPath: modelsRoot))
        default:
            return .notReady("notConfigured")
        }
    }

    // MARK: Running

    func run(requestJSON: String, jobHandle: UInt64) -> TranscriptionResponse {
        defer { finish(jobHandle: jobHandle) }

        guard let data = requestJSON.data(using: .utf8),
            let request = try? JSONDecoder().decode(TranscriptionRequest.self, from: data)
        else {
            return .failure(.failed)
        }

        let audioURL = URL(fileURLWithPath: request.audioPath)
        guard FileManager.default.fileExists(atPath: audioURL.path) else {
            return .failure(.audioUnreadable)
        }

        let context = AdapterContext(
            request: request,
            audioURL: audioURL,
            modelsRoot: URL(fileURLWithPath: request.modelsRoot),
            isCancelled: { [weak self] in self?.isCancelled(jobHandle) ?? false },
            reportProgress: { [weak self] fraction in
                self?.report(jobHandle: jobHandle, fraction: fraction)
            }
        )

        do {
            let result = try runBlocking(context)
            if context.isCancelled() {
                return .failure(.cancelled)
            }
            return .success(
                words: result.words, speakerSpans: result.speakerSpans,
                modelId: result.modelId, language: result.language)
        } catch let error as AdapterError {
            return .failure(error.code)
        } catch {
            return .failure(.failed)
        }
    }

    /// Bridges the adapters' `async` APIs to the synchronous C ABI. Safe
    /// because Rust always calls `blueear_transcription_run` from a background
    /// worker thread, never from the main thread, so blocking here cannot
    /// deadlock the UI.
    private func runBlocking(_ context: AdapterContext) throws -> AdapterResult {
        let semaphore = DispatchSemaphore(value: 0)
        let box = ResultBox()

        Task.detached(priority: .userInitiated) {
            do {
                box.value = .success(try await Self.dispatch(context))
            } catch {
                box.value = .failure(error)
            }
            semaphore.signal()
        }
        semaphore.wait()

        switch box.value {
        case .success(let result): return result
        case .failure(let error): throw error
        case nil: throw AdapterError(code: .failed)
        }
    }

    private static func dispatch(_ context: AdapterContext) async throws -> AdapterResult {
        switch context.request.provider {
        case "apple_speech":
            return try await AppleSpeechAdapter.transcribe(context)
        case "fluidaudio":
            return try await FluidAudioAdapter.transcribe(context)
        default:
            throw AdapterError(code: .providerNotReady)
        }
    }
}

/// Everything an adapter needs for one track, including the two callbacks that
/// let it stay ignorant of job handles.
struct AdapterContext: Sendable {
    let request: TranscriptionRequest
    let audioURL: URL
    let modelsRoot: URL
    let isCancelled: @Sendable () -> Bool
    let reportProgress: @Sendable (Double) -> Void
}

extension TranscriptionRequest: @unchecked Sendable {}

/// Carries a result out of the detached task the semaphore is waiting on.
private final class ResultBox: @unchecked Sendable {
    var value: Result<AdapterResult, Error>?
}
