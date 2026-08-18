import { describe, expect, it } from "vitest";

import {
  canTranscribe,
  exportFilename,
  formatBundleSize,
  formatTimecode,
  isJobResumable,
  isJobRunning,
  notReadyLabel,
  speakerLabel,
} from "./transcriptionUi";
import type {
  Job,
  JobStatus,
  NotReadyReason,
  ProviderStatus,
  TranscriptionOverview,
} from "../types/transcription";

function provider(overrides: Partial<ProviderStatus> = {}): ProviderStatus {
  return {
    id: "fluidaudio",
    displayName: "FluidAudio",
    minimumMacos: "14.4",
    minimumWindows: "",
    requiresModelImport: true,
    supportsRemoteSpeakerLabels: true,
    supportsWordTimings: true,
    summary: "Runs imported speech models on this Mac.",
    ready: true,
    notReadyReason: null,
    ...overrides,
  };
}

function overview(overrides: Partial<TranscriptionOverview> = {}): TranscriptionOverview {
  return {
    providers: [provider()],
    anyProviderInstallable: true,
    preferences: {
      schemaVersion: 1,
      provider: "fluidaudio",
      autoTranscribe: false,
      diarizeRemoteSpeakers: true,
      language: null,
    },
    installedBundles: [],
    ...overrides,
  };
}

function job(status: JobStatus, trackStatuses: JobStatus[]): Job {
  return {
    schemaVersion: 1,
    sessionId: "session-a",
    provider: "fluidaudio",
    status,
    progress: 0.5,
    tracks: trackStatuses.map((trackStatus, index) => ({
      track: index === 0 ? "meeting" : "microphone",
      status: trackStatus,
      progress: trackStatus === "completed" ? 1 : 0,
      error: null,
    })),
    attempt: 1,
    diarize: true,
    startedAt: "2026-08-07T10:00:00Z",
    updatedAt: "2026-08-07T10:01:00Z",
    error: null,
  };
}

describe("speakerLabel", () => {
  it("matches the labels the Rust exports use", () => {
    expect(speakerLabel({ kind: "you" })).toBe("You");
    expect(speakerLabel({ kind: "meetingAudio" })).toBe("Meeting audio");
    expect(speakerLabel({ kind: "remote", index: 2 })).toBe("Speaker 2");
    expect(speakerLabel({ kind: "unknown" })).toBe("Unknown speaker");
  });
});

describe("notReadyLabel", () => {
  it("tells the user what to do rather than what failed", () => {
    expect(notReadyLabel(provider({ notReadyReason: "modelsMissing" }))).toContain(
      "Get the models",
    );
    expect(notReadyLabel(provider({ notReadyReason: "languageAssetsMissing" }))).toContain(
      "System Settings",
    );
  });

  it("names the version a too-old OS needs", () => {
    const status = provider({ notReadyReason: "osTooOld", minimumMacos: "26", minimumWindows: "" });
    expect(notReadyLabel(status)).toBe("Needs macOS 26+.");
  });

  it("says nothing when the provider is ready", () => {
    expect(notReadyLabel(provider())).toBe("");
  });

  it("has copy for every reason the backend can report", () => {
    const reasons: NotReadyReason[] = [
      "notConfigured",
      "osTooOld",
      "notBuilt",
      "modelsMissing",
      "languageAssetsMissing",
      "probeFailed",
    ];
    for (const reason of reasons) {
      expect(notReadyLabel(provider({ notReadyReason: reason }))).not.toBe("");
    }
  });
});

describe("job predicates", () => {
  it("treats only non-terminal jobs as running", () => {
    expect(isJobRunning(job("transcribing", ["transcribing"]))).toBe(true);
    expect(isJobRunning(job("merging", ["completed"]))).toBe(true);
    expect(isJobRunning(job("completed", ["completed"]))).toBe(false);
    expect(isJobRunning(null)).toBe(false);
  });

  it("offers retry only when work is actually left to redo", () => {
    expect(isJobResumable(job("failed", ["completed", "failed"]))).toBe(true);
    expect(isJobResumable(job("cancelled", ["completed", "cancelled"]))).toBe(true);
    expect(isJobResumable(job("completed", ["completed", "completed"]))).toBe(false);
    expect(isJobResumable(job("transcribing", ["transcribing"]))).toBe(false);
  });
});

describe("canTranscribe", () => {
  it("requires a selected provider that is actually ready", () => {
    expect(canTranscribe(overview())).toBe(true);
    expect(canTranscribe(overview({ providers: [provider({ ready: false })] }))).toBe(false);
  });

  it("is false for the default recording-only configuration", () => {
    const recordingOnly = overview({
      providers: [provider({ id: "none", ready: false, notReadyReason: "notConfigured" })],
      preferences: { ...overview().preferences, provider: "none" },
    });
    expect(canTranscribe(recordingOnly)).toBe(false);
  });

  it("is false before the overview has loaded", () => {
    expect(canTranscribe(null)).toBe(false);
  });
});

describe("formatting", () => {
  it("reports bundle sizes at a human scale", () => {
    expect(formatBundleSize(1_500_000_000)).toBe("1.4 GB");
    expect(formatBundleSize(250 * 1024 ** 2)).toBe("250 MB");
  });

  it("renders segment timecodes as minutes and seconds", () => {
    expect(formatTimecode(0)).toBe("0:00");
    expect(formatTimecode(75.9)).toBe("1:15");
    expect(formatTimecode(-3)).toBe("0:00");
  });

  it("names the files written beside a recording", () => {
    expect(exportFilename("text")).toBe("transcript.txt");
    expect(exportFilename("vtt")).toBe("transcript.vtt");
  });
});
