// AppleSpeechAdapter.swift
//
// `SpeechAnalyzer` / `SpeechTranscriber`, the on-device speech stack Apple
// introduced in macOS 26.
//
// The API only exists in the macOS 26 SDK, so the whole implementation sits
// behind `#if compiler(>=6.2)` -- Swift 6.2 is the toolchain that ships that
// SDK. Built with anything older, this file compiles to a stub that reports
// `notBuilt`, the provider is unselectable in settings, and the rest of the
// app is unaffected. A runtime `@available` check is still required on top,
// because a new-SDK build can perfectly well run on macOS 15.
//
// This provider transcribes what was said, not who said it: Apple's API
// exposes no diarization. Teams-track segments therefore stay labelled
// "Meeting audio". That is the honest trade against FluidAudio, and the
// settings screen shows both so the user can choose with it in view.

import Foundation

enum AppleSpeechAdapter {

    #if compiler(>=6.2)

    static func probe() -> TranscriptionProbe {
        guard #available(macOS 26.0, *) else {
            return .notReady("osTooOld")
        }
        return AppleSpeechModern.probe()
    }

    static func transcribe(_ context: AdapterContext) async throws -> AdapterResult {
        guard #available(macOS 26.0, *) else {
            throw AdapterError(code: .unsupportedOS)
        }
        return try await AppleSpeechModern.transcribe(context)
    }

    #else

    static func probe() -> TranscriptionProbe {
        .notReady("notBuilt")
    }

    static func transcribe(_ context: AdapterContext) async throws -> AdapterResult {
        throw AdapterError(code: .notBuilt)
    }

    #endif
}

#if compiler(>=6.2)

import AVFoundation
import Speech

@available(macOS 26.0, *)
enum AppleSpeechModern {

    /// Ready only when macOS already has speech assets installed for a locale
    /// the transcriber supports. Blue Ear never triggers the download itself:
    /// the app requests no network access, and quietly pulling hundreds of
    /// megabytes because someone opened a settings screen would be exactly the
    /// kind of surprise the design rules out. The UI points at System Settings
    /// instead.
    static func probe() -> TranscriptionProbe {
        let semaphore = DispatchSemaphore(value: 0)
        let box = ProbeBox()

        Task.detached(priority: .userInitiated) {
            let installed = await SpeechTranscriber.installedLocales
            let current = Locale.current
            box.hasAssets = installed.contains { $0.identifier(.bcp47) == current.identifier(.bcp47) }
                || !installed.isEmpty
            semaphore.signal()
        }
        semaphore.wait()

        return box.hasAssets ? .ok : .notReady("languageAssetsMissing")
    }

    static func transcribe(_ context: AdapterContext) async throws -> AdapterResult {
        let locale = try await resolveLocale()

        let transcriber = SpeechTranscriber(
            locale: locale,
            transcriptionOptions: [],
            reportingOptions: [],
            attributeOptions: [.audioTimeRange]
        )
        let analyzer = SpeechAnalyzer(modules: [transcriber])

        let audioFile: AVAudioFile
        do {
            audioFile = try AVAudioFile(forReading: context.audioURL)
        } catch {
            throw AdapterError(code: .audioUnreadable)
        }

        let totalSeconds = max(
            Double(audioFile.length) / audioFile.processingFormat.sampleRate, 0.001)

        // Results stream in while the analyzer consumes the file, so the
        // collector has to be running before analysis starts.
        let collector = Task { () -> [TranscribedWord] in
            var words: [TranscribedWord] = []
            for try await result in transcriber.results {
                if context.isCancelled() { break }
                words.append(contentsOf: Self.words(in: result.text))
                if let last = words.last {
                    context.reportProgress(min(last.endSeconds / totalSeconds, 1.0))
                }
            }
            return words
        }

        do {
            try await analyzer.analyzeSequence(from: audioFile)
            try await analyzer.finalizeAndFinishThroughEndOfInput()
        } catch {
            collector.cancel()
            throw AdapterError(code: context.isCancelled() ? .cancelled : .failed)
        }

        let words = try await collector.value
        if context.isCancelled() {
            throw AdapterError(code: .cancelled)
        }
        context.reportProgress(1.0)

        return AdapterResult(
            words: words,
            // Apple's API has no diarization. Returning an empty span list is
            // what tells Rust to fall back to track-level labelling.
            speakerSpans: [],
            modelId: "apple-speech-analyzer",
            language: locale.identifier(.bcp47)
        )
    }

    private static func resolveLocale() async throws -> Locale {
        if let supported = await SpeechTranscriber.supportedLocale(equivalentTo: Locale.current),
            await SpeechTranscriber.installedLocales.contains(where: {
                $0.identifier(.bcp47) == supported.identifier(.bcp47)
            })
        {
            return supported
        }
        guard let fallback = await SpeechTranscriber.installedLocales.first else {
            throw AdapterError(code: .providerNotReady)
        }
        return fallback
    }

    /// Splits one transcription result into words, using the audio time range
    /// each attributed run carries. Runs can span several words, so a run's
    /// duration is divided evenly across the words inside it -- coarser than
    /// FluidAudio's per-token timings, but enough to align a transcript to
    /// playback, which is all these timestamps are used for.
    private static func words(in text: AttributedString) -> [TranscribedWord] {
        var words: [TranscribedWord] = []

        for run in text.runs {
            guard let range = run.audioTimeRange else { continue }
            let start = range.start.seconds
            let end = range.end.seconds
            let pieces = String(text[run.range].characters)
                .split(whereSeparator: { $0.isWhitespace })
            guard !pieces.isEmpty else { continue }

            let step = (end - start) / Double(pieces.count)
            for (index, piece) in pieces.enumerated() {
                words.append(
                    TranscribedWord(
                        text: String(piece),
                        startSeconds: start + step * Double(index),
                        endSeconds: start + step * Double(index + 1),
                        confidence: nil
                    ))
            }
        }

        return words
    }
}

@available(macOS 26.0, *)
private final class ProbeBox: @unchecked Sendable {
    var hasAssets = false
}

#endif
