import type {
  MeetingAppId,
  Readiness,
  SessionMetadata,
  SessionState,
} from "../types/recording";
import type { RecordingTrack } from "../types/recording";

export function canStartRecording(
  readiness: Readiness | null,
  sessionState: SessionState,
  sourceApp: MeetingAppId | null,
): boolean {
  if (!readiness || !sourceApp || sessionState.state !== "idle") return false;
  if (!readiness.osSupported || readiness.permissionState !== "granted") return false;
  const app = readiness.meetingApps.find((a) => a.id === sourceApp);
  return !!app?.installed && !!app?.running;
}

export function canEnableMicrophone(readiness: Readiness | null): boolean {
  return readiness?.microphoneAvailable ?? false;
}

/** Prefer the single running app when unambiguous; otherwise null. */
export function autoSelectSourceApp(readiness: Readiness | null): MeetingAppId | null {
  if (!readiness) return null;
  const running = readiness.meetingApps.filter((a) => a.running);
  if (running.length === 1) return running[0].id;
  return null;
}

export function availableTracks(session: SessionMetadata): RecordingTrack[] {
  const tracks: RecordingTrack[] = [];
  if (session.meetingWav) tracks.push("meeting");
  if (session.micEnabled && session.microphoneWav) tracks.push("microphone");
  tracks.push("mixed");
  return tracks;
}
