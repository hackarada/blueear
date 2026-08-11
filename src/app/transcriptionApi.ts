// The transcription half of the Tauri IPC surface, following the same rules as
// recordingApi.ts: one function per command, camelCase arguments, and types
// from types/transcription.ts.
//
// Note what is absent. There is no command that takes a filesystem path. Model
// bundles are chosen by a native panel and transcripts are written beside the
// recording the session ID resolves to, so no path ever originates here.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

import type {
  ExportFormat,
  Job,
  ModelDownloadPage,
  Transcript,
  TranscriptionOverview,
  TranscriptionPreferences,
} from "../types/transcription";

export function getTranscriptionOverview(): Promise<TranscriptionOverview> {
  return invoke("get_transcription_overview");
}

export function setTranscriptionPreferences(
  preferences: TranscriptionPreferences,
): Promise<TranscriptionOverview> {
  return invoke("set_transcription_preferences", { preferences });
}

export function startTranscription(sessionId: string): Promise<Job> {
  return invoke("start_transcription", { sessionId });
}

export function retryTranscription(sessionId: string): Promise<Job> {
  return invoke("retry_transcription", { sessionId });
}

export function cancelTranscription(sessionId: string): Promise<void> {
  return invoke("cancel_transcription", { sessionId });
}

export function getTranscriptionJob(sessionId: string): Promise<Job | null> {
  return invoke("get_transcription_job", { sessionId });
}

export function getTranscript(sessionId: string): Promise<Transcript> {
  return invoke("get_transcript", { sessionId });
}

export function exportTranscript(sessionId: string, format: ExportFormat): Promise<string> {
  return invoke("export_transcript", { sessionId, format });
}

// Resolves to null when the user dismissed the native picker.
export function importModelBundle(): Promise<TranscriptionOverview | null> {
  return invoke("import_model_bundle");
}

export function deleteModelBundle(bundleId: string): Promise<TranscriptionOverview> {
  return invoke("delete_model_bundle", { bundleId });
}

// Opens an allowlisted Hugging Face page in the system browser. The URL is
// chosen entirely on the Rust side from the page kind; this never invents one.
export function openModelDownloadPage(page: ModelDownloadPage): Promise<void> {
  return invoke("open_model_download_page", { page });
}

export function onTranscriptionJob(handler: (job: Job) => void): Promise<UnlistenFn> {
  return listen<Job>("transcription-job", (event) => handler(event.payload));
}
