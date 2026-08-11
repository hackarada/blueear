// Mirrors the Rust types in src-tauri/src/session/{state,manager}.rs,
// src-tauri/src/storage/session_store.rs, and src-tauri/src/error.rs.
// Keep in sync by hand -- there are only a handful of shapes.

export type PermissionState = "unknown" | "granted" | "denied" | "needsProbe";

export type MeetingAppId = "teams" | "zoom";

export interface MeetingAppStatus {
  id: MeetingAppId;
  displayName: string;
  installed: boolean;
  running: boolean;
}

export interface Readiness {
  osSupported: boolean;
  platform: "macos" | "windows" | "other" | string;
  recordingsPathDisplay: string;
  meetingApps: MeetingAppStatus[];
  microphoneAvailable: boolean;
  permissionState: PermissionState;
}

export interface SessionMetadata {
  schemaVersion: number;
  sessionId: string;
  startedAt: string;
  endedAt: string;
  durationSeconds: number;
  micEnabled: boolean;
  recovered: boolean;
  interrupted: boolean;
  droppedMeetingFrames: number;
  droppedMicFrames: number;
  sampleRate: number;
  meetingWav: string | null;
  microphoneWav: string | null;
  mixedWav: string;
  sourceApp: MeetingAppId;
  appBundleId: string;
  appVersion: string;
}

export type ErrorCode =
  | "UNSUPPORTED_OS"
  | "MEETING_APP_NOT_FOUND"
  | "MEETING_APP_NOT_RUNNING"
  | "AUDIO_PERMISSION_DENIED"
  | "MIC_PERMISSION_DENIED"
  | "MIC_UNAVAILABLE"
  | "SOURCE_SILENT"
  | "DISK_FULL"
  | "FINALIZE_FAILED"
  | "SESSION_CONFLICT"
  | "SESSION_NOT_FOUND"
  | "TRACK_NOT_FOUND"
  | "TRANSCRIPTION_UNAVAILABLE"
  | "TRANSCRIPTION_PROVIDER_NOT_READY"
  | "TRANSCRIPTION_MODEL_MISSING"
  | "TRANSCRIPTION_INVALID_BUNDLE"
  | "TRANSCRIPTION_CANCELLED"
  | "TRANSCRIPTION_INTERRUPTED"
  | "TRANSCRIPTION_FAILED"
  | "TRANSCRIPT_NOT_FOUND"
  | "INTERNAL";

/** Playback track keys. `"teams"` is accepted as a legacy alias for meeting. */
export type RecordingTrack = "meeting" | "teams" | "microphone" | "mixed";

export interface AppError {
  code: ErrorCode;
  message: string;
}

export type SessionState =
  | { state: "idle" }
  | { state: "preparing"; sessionId: string }
  | {
      state: "recording";
      sessionId: string;
      startedAtMs: number;
      micEnabled: boolean;
      sourceApp: MeetingAppId;
    }
  | {
      state: "recovering";
      sessionId: string;
      startedAtMs: number;
      micEnabled: boolean;
      sourceApp: MeetingAppId;
    }
  | { state: "stopping"; sessionId: string }
  | { state: "finalizing"; sessionId: string }
  | { state: "completed"; metadata: SessionMetadata }
  | { state: "failed"; error: AppError };

export interface LevelsEvent {
  meeting: number;
  microphone: number;
}

export interface SourceWarningEvent {
  code: string;
}

export function isAppError(value: unknown): value is AppError {
  return (
    typeof value === "object" &&
    value !== null &&
    "code" in value &&
    "message" in value
  );
}

export function displayNameForApp(id: MeetingAppId): string {
  switch (id) {
    case "teams":
      return "Microsoft Teams";
    case "zoom":
      return "Zoom";
  }
}
