// FluidAudioAdapter.swift
//
// Parakeet TDT ASR plus offline speaker diarization, both from FluidAudio,
// loaded exclusively from Blue Ear's own validated model directory.
//
// FluidAudio ships inside the app binary but is runtime-optional: nothing in
// this file executes -- no model is loaded, no CoreML graph is compiled, no
// Neural Engine resource is allocated -- unless the user selected FluidAudio
// and it passed its readiness check. `probe` deliberately answers from the
// filesystem alone for exactly that reason.
//
// SECURITY-REVIEW: `ModelHub.offlineMode` is set to `true` before any other
// FluidAudio type is touched, which makes every download path in the library
// throw `DownloadError.networkDisabled` instead of reaching HuggingFace. Model
// files are read only from `modelsRoot`, the app-owned directory that
// `model_import.rs` promotes validated bundles into; the user's original
// bundle path is never opened here.

import AVFoundation
import FluidAudio
import Foundation

enum FluidAudioAdapter {

    /// Local cache directory names FluidAudio's loaders expect.
    ///
    /// `AsrModels.load(from:)` takes a path whose *parent* is the models root,
    /// then resolves `Repo.folderName` under that parent. For Parakeet v3,
    /// `folderName` strips the `-coreml` Hugging Face suffix, so the on-disk
    /// ASR directory must be `parakeet-tdt-0.6b-v3` (not
    /// `parakeet-tdt-0.6b-v3-coreml`). Diarization uses explicit file paths, so
    /// it keeps the repo folder name. `model_import`'s allowlist pins the same
    /// strings.
    static let asrRepoDirectory = "parakeet-tdt-0.6b-v3"
    static let diarizerRepoDirectory = "speaker-diarization-coreml"

    /// The sample rate both FluidAudio pipelines expect.
    private static let targetSampleRate = 16_000

    /// Answers from the filesystem only. Loading a model to find out whether a
    /// model can be loaded would defeat the whole point of keeping FluidAudio
    /// inert until it is chosen.
    static func probe(modelsRoot: URL) -> TranscriptionProbe {
        let asr = modelsRoot.appendingPathComponent(asrRepoDirectory)
        let diarizer = modelsRoot.appendingPathComponent(diarizerRepoDirectory)

        let required = [
            asr.appendingPathComponent(ModelNames.ASR.preprocessorFile),
            asr.appendingPathComponent(ParakeetEncoderPrecision.int8.encoderFileName),
            asr.appendingPathComponent(ModelNames.ASR.decoderFile),
            asr.appendingPathComponent(ModelNames.ASR.jointV3File),
            asr.appendingPathComponent(ModelNames.ASR.vocabularyFile),
            diarizer.appendingPathComponent(ModelNames.Diarizer.segmentationFile),
            diarizer.appendingPathComponent(ModelNames.Diarizer.embeddingFile),
        ]

        let allPresent = required.allSatisfy { FileManager.default.fileExists(atPath: $0.path) }
        return allPresent ? .ok : .notReady("modelsMissing")
    }

    static func transcribe(_ context: AdapterContext) async throws -> AdapterResult {
        // Must happen before any other FluidAudio call, per the library's own
        // contract for `offlineMode`.
        ModelHub.offlineMode = true

        guard probe(modelsRoot: context.modelsRoot).ready else {
            throw AdapterError(code: .modelsMissing)
        }
        try checkCancelled(context)

        let words = try await transcribeWords(context)
        try checkCancelled(context)

        let speakerSpans =
            context.request.diarize ? try diarize(context) : []
        try checkCancelled(context)

        context.reportProgress(1.0)
        return AdapterResult(
            words: words,
            speakerSpans: speakerSpans,
            modelId: asrRepoDirectory,
            language: context.request.language
        )
    }

    // MARK: - ASR

    private static func transcribeWords(_ context: AdapterContext) async throws -> [TranscribedWord]
    {
        let asrDirectory = context.modelsRoot.appendingPathComponent(asrRepoDirectory)

        let models: AsrModels
        do {
            models = try await AsrModels.load(
                from: asrDirectory, version: .v3, encoderPrecision: .int8)
        } catch {
            throw AdapterError(code: .modelsMissing)
        }
        try checkCancelled(context)
        context.reportProgress(0.1)

        let manager = AsrManager(config: ASRConfig())
        do {
            try await manager.loadModels(models)
        } catch {
            throw AdapterError(code: .modelsMissing)
        }
        defer { Task { await manager.cleanup() } }

        try checkCancelled(context)
        context.reportProgress(0.2)

        var decoderState = TdtDecoderState.make(decoderLayers: models.version.decoderLayers)
        let result: ASRResult
        do {
            result = try await manager.transcribe(context.audioURL, decoderState: &decoderState)
        } catch {
            throw AdapterError(code: context.isCancelled() ? .cancelled : .failed)
        }

        // Diarization, when requested, is the second half of the work.
        context.reportProgress(context.request.diarize ? 0.6 : 0.95)

        // FluidAudio emits SentencePiece sub-word tokens; its own boundary
        // rules are the right thing to group them with, so Rust only ever sees
        // whole words. Confidence is left unset: what FluidAudio reports is an
        // utterance-level score, and stamping it onto every word would look
        // like per-word confidence without being it.
        return buildWordTimings(from: result.tokenTimings ?? []).map {
            TranscribedWord(
                text: $0.word,
                startSeconds: $0.startTime,
                endSeconds: $0.endTime,
                confidence: nil
            )
        }
    }

    // MARK: - Diarization

    /// Runs offline diarization over the whole track and returns raw speaker
    /// spans. No attempt is made to match words to speakers here: that
    /// alignment is a pure function that belongs in Rust, where it is unit
    /// tested against ambiguous and non-overlapping cases.
    private static func diarize(_ context: AdapterContext) throws -> [TranscribedSpeakerSpan] {
        let directory = context.modelsRoot.appendingPathComponent(diarizerRepoDirectory)

        let models: DiarizerModels
        do {
            models = try DiarizerModels.load(
                localSegmentationModel: directory.appendingPathComponent(
                    ModelNames.Diarizer.segmentationFile),
                localEmbeddingModel: directory.appendingPathComponent(
                    ModelNames.Diarizer.embeddingFile)
            )
        } catch {
            throw AdapterError(code: .modelsMissing)
        }
        try checkCancelled(context)

        let samples: [Float]
        do {
            samples = try AudioConverter(sampleRate: Double(targetSampleRate))
                .resampleAudioFile(context.audioURL)
        } catch {
            throw AdapterError(code: .audioUnreadable)
        }
        try checkCancelled(context)

        let diarizer = DiarizerManager()
        diarizer.initialize(models: consume models)
        defer { diarizer.cleanup() }

        let result: DiarizationResult
        do {
            result = try diarizer.performCompleteDiarization(
                samples,
                sampleRate: targetSampleRate,
                progressHandler: { fraction in
                    context.reportProgress(0.6 + 0.35 * fraction)
                }
            )
        } catch {
            throw AdapterError(code: context.isCancelled() ? .cancelled : .failed)
        }

        return result.segments.map {
            TranscribedSpeakerSpan(
                speakerKey: $0.speakerId,
                startSeconds: Double($0.startTimeSeconds),
                endSeconds: Double($0.endTimeSeconds)
            )
        }
    }

    // MARK: - Helpers

    private static func checkCancelled(_ context: AdapterContext) throws {
        if context.isCancelled() {
            throw AdapterError(code: .cancelled)
        }
    }
}
