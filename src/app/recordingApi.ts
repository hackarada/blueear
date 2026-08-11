// Thin wrapper around the Tauri IPC surface. No component talks to
// `@tauri-apps/api` directly -- everything goes through here so the typed
// contract in `types/recording.ts` stays the single source of truth.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  LevelsEvent,
  MeetingAppId,
  PermissionState,
  Readiness,
  RecordingTrack,
  SessionMetadata,
  SessionState,
  SourceWarningEvent,
} from "../types/recording";

export function getReadiness(): Promise<Readiness> {
  return invoke("get_readiness");
}

export function requestCaptureAccess(): Promise<PermissionState> {
  return invoke("request_capture_access");
}

export function startRecording(
  sourceApp: MeetingAppId,
  includeMicrophone: boolean,
): Promise<string> {
  return invoke("start_recording", { sourceApp, includeMicrophone });
}

export function stopRecording(sessionId: string): Promise<SessionMetadata> {
  return invoke("stop_recording", { sessionId });
}

export function getSessionState(): Promise<SessionState> {
  return invoke("get_session_state");
}

export function revealSession(sessionId: string): Promise<void> {
  return invoke("reveal_session", { sessionId });
}

export function listRecentSessions(limit: number): Promise<SessionMetadata[]> {
  return invoke("list_recent_sessions", { limit });
}

export function getSessionAssetPath(sessionId: string, track: RecordingTrack): Promise<string> {
  return invoke("get_session_asset_path", { sessionId, track });
}

export function dismissSession(): Promise<void> {
  return invoke("dismiss_session");
}

export function onSessionState(handler: (state: SessionState) => void): Promise<UnlistenFn> {
  return listen<SessionState>("session-state", (event) => handler(event.payload));
}

export function onLevels(handler: (levels: LevelsEvent) => void): Promise<UnlistenFn> {
  return listen<LevelsEvent>("levels", (event) => handler(event.payload));
}

export function onSourceWarning(handler: (warning: SourceWarningEvent) => void): Promise<UnlistenFn> {
  return listen<SourceWarningEvent>("source-warning", (event) => handler(event.payload));
}

export function onNavigate(handler: (screen: string) => void): Promise<UnlistenFn> {
  return listen<{ screen: string }>("navigate", (event) => handler(event.payload.screen));
}
