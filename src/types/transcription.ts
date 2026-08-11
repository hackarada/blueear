// Mirrors the Rust types in src-tauri/src/transcription/{types,store,service,
// model_import}.rs. Keep in sync by hand, as with types/recording.ts.
//
// Note the wire casing: `ProviderId` is snake_case because those identifiers
// also name directories and native adapters, while everything else is
// camelCase.

import type { AppError } from "./recording";

export type ProviderId = "none" | "apple_speech" | "fluidaudio" | "whisper";

// Narrower than RecordingTrack: mixed.wav is never transcribed.
// Wire value is `meeting`; legacy transcripts may still say `teams`.
export type TranscriptTrack = "meeting" | "teams" | "microphone";

export type Speaker =
  | { kind: "you" }
  | { kind: "meetingAudio" }
  | { kind: "remote"; index: number }
  | { kind: "unknown" };

export interface Word {
  text: string;
  startSeconds: number;
  endSeconds: number;
  confidence: number | null;
}

export interface Segment {
  track: TranscriptTrack;
  speaker: Speaker;
  startSeconds: number;
  endSeconds: number;
  text: string;
  words: Word[];
}

export interface TrackTranscript {
  track: TranscriptTrack;
  provider: ProviderId;
  modelId: string | null;
  language: string | null;
  diarized: boolean;
  speakerCount: number;
  segments: Segment[];
}

export interface Transcript {
  schemaVersion: number;
  sessionId: string;
  provider: ProviderId;
  createdAt: string;
  durationSeconds: number;
  tracks: TrackTranscript[];
  segments: Segment[];
}

export type JobStatus =
  | "queued"
  | "preparing"
  | "transcribing"
  | "merging"
  | "completed"
  | "cancelled"
  | "failed";

export interface TrackJob {
  track: TranscriptTrack;
  status: JobStatus;
  progress: number;
  error: AppError | null;
}

export interface Job {
  schemaVersion: number;
  sessionId: string;
  provider: ProviderId;
  status: JobStatus;
  progress: number;
  tracks: TrackJob[];
  attempt: number;
  diarize: boolean;
  startedAt: string;
  updatedAt: string;
  error: AppError | null;
}

export type NotReadyReason =
  | "notConfigured"
  | "osTooOld"
  | "notBuilt"
  | "modelsMissing"
  | "languageAssetsMissing"
  | "probeFailed";

// ProviderStatus flattens its capabilities in Rust, so this is one flat object
// rather than a nested one.
export interface ProviderStatus {
  id: ProviderId;
  displayName: string;
  minimumMacos: string;
  minimumWindows: string;
  requiresModelImport: boolean;
  supportsRemoteSpeakerLabels: boolean;
  supportsWordTimings: boolean;
  summary: string;
  ready: boolean;
  notReadyReason: NotReadyReason | null;
}

export interface TranscriptionPreferences {
  schemaVersion: number;
  provider: ProviderId;
  autoTranscribe: boolean;
  diarizeRemoteSpeakers: boolean;
  language: string | null;
}

export interface ManifestModel {
  id: string;
  role: string;
  license: string;
}

export interface InstalledBundle {
  bundleId: string;
  displayName: string;
  provider: ProviderId;
  sdkVersion: string;
  models: ManifestModel[];
  totalBytes: number;
  installedAt: string;
}

// One snapshot of the whole settings surface, so readiness, selection, and
// installed bundles can never be rendered out of step with each other.
export interface TranscriptionOverview {
  providers: ProviderStatus[];
  anyProviderInstallable: boolean;
  preferences: TranscriptionPreferences;
  installedBundles: InstalledBundle[];
}

export type ExportFormat = "text" | "vtt";

export type ModelDownloadPage = "asr" | "diarization";
