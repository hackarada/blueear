// Pure presentation logic for transcription: the copy that turns a readiness
// reason or a job status into something a person can act on, kept out of the
// components so it can be tested directly.

import type {
  ExportFormat,
  Job,
  JobStatus,
  ProviderStatus,
  Speaker,
  TranscriptionOverview,
  TranscriptTrack,
} from "../types/transcription";

export const TRANSCRIPT_TRACK_LABELS: Record<TranscriptTrack, string> = {
  meeting: "Meeting audio",
  teams: "Meeting audio",
  microphone: "Your microphone",
};

// Mirrors Speaker::display_name in src-tauri/src/transcription/types.rs so the
// on-screen labels and the exported file agree exactly.
export function speakerLabel(speaker: Speaker): string {
  switch (speaker.kind) {
    case "you":
      return "You";
    case "meetingAudio":
      return "Meeting audio";
    case "remote":
      return `Speaker ${speaker.index}`;
    case "unknown":
      return "Unknown speaker";
  }
}

// Says what the user has to do, not what the system observed. A reason with no
// user action, like a probe failure, is the one case that gets an apology.
export function notReadyLabel(provider: ProviderStatus): string {
  switch (provider.notReadyReason) {
    case "notConfigured":
      return "Recording only. No transcript is created.";
    case "osTooOld": {
      const parts: string[] = [];
      if (provider.minimumMacos) parts.push(`macOS ${provider.minimumMacos}+`);
      if (provider.minimumWindows) parts.push(`Windows ${provider.minimumWindows}+`);
      return parts.length > 0
        ? `Needs ${parts.join(" or ")}.`
        : "This operating system is too old for this provider.";
    }
    case "notBuilt":
      return "Not available in this build of Blue Ear.";
    case "modelsMissing":
      return "Get the models below, pack them into a Blue Ear bundle, then Import.";
    case "languageAssetsMissing":
      return "Download a language in System Settings > General > Language & Region.";
    case "probeFailed":
      return "Couldn't check whether this provider is ready.";
    default:
      return "";
  }
}

export function jobStatusLabel(status: JobStatus): string {
  switch (status) {
    case "queued":
      return "Queued";
    case "preparing":
      return "Preparing";
    case "transcribing":
      return "Transcribing";
    case "merging":
      return "Merging tracks";
    case "completed":
      return "Transcribed";
    case "cancelled":
      return "Cancelled";
    case "failed":
      return "Failed";
  }
}

export function isJobRunning(job: Job | null): boolean {
  if (!job) return false;
  return !["completed", "cancelled", "failed"].includes(job.status);
}

// A job that ended without transcribing everything can be resumed; retry only
// redoes the tracks that did not finish.
export function isJobResumable(job: Job | null): boolean {
  if (!job) return false;
  return (
    (job.status === "failed" || job.status === "cancelled") &&
    job.tracks.some((track) => track.status !== "completed")
  );
}

export function selectedProvider(overview: TranscriptionOverview | null): ProviderStatus | null {
  if (!overview) return null;
  return overview.providers.find((p) => p.id === overview.preferences.provider) ?? null;
}

// Whether the Transcribe action should be offered at all. Offering it while the
// chosen provider is unready would only produce an error the user cannot act on
// from the results screen.
export function canTranscribe(overview: TranscriptionOverview | null): boolean {
  const provider = selectedProvider(overview);
  return !!provider && provider.id !== "none" && provider.ready;
}

export function formatBundleSize(bytes: number): string {
  const gigabytes = bytes / 1024 ** 3;
  if (gigabytes >= 1) return `${gigabytes.toFixed(1)} GB`;
  return `${Math.max(1, Math.round(bytes / 1024 ** 2))} MB`;
}

export function formatTimecode(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const minutes = Math.floor(total / 60);
  return `${minutes}:${(total % 60).toString().padStart(2, "0")}`;
}

export function exportFilename(format: ExportFormat): string {
  switch (format) {
    case "text":
      return "transcript.txt";
    case "vtt":
      return "transcript.vtt";
    default: {
      const _never: never = format;
      return _never;
    }
  }
}
