import { describe, expect, it } from "vitest";

import type { Readiness, SessionMetadata } from "../types/recording";

import {
  availableTracks,
  canEnableMicrophone,
  canStartRecording,
} from "./recordingUi";

const ready: Readiness = {
  osSupported: true,
  platform: "macos",
  recordingsPathDisplay: "~/Music/BlueEar/Recordings",
  meetingApps: [
    {
      id: "teams",
      displayName: "Microsoft Teams",
      installed: true,
      running: true,
    },
    {
      id: "zoom",
      displayName: "Zoom",
      installed: true,
      running: false,
    },
  ],
  microphoneAvailable: true,
  permissionState: "granted",
};

describe("canStartRecording", () => {
  it("is true when every readiness gate passes and state is idle", () => {
    expect(canStartRecording(ready, { state: "idle" }, "teams")).toBe(true);
  });

  it("is false without system audio permission", () => {
    expect(
      canStartRecording({ ...ready, permissionState: "denied" }, { state: "idle" }, "teams"),
    ).toBe(false);
  });

  it("is false while a session is active", () => {
    expect(
      canStartRecording(
        ready,
        {
          state: "recording",
          sessionId: "s",
          startedAtMs: 0,
          micEnabled: false,
          sourceApp: "teams",
        },
        "teams",
      ),
    ).toBe(false);
  });

  it("is false when the selected app is not running", () => {
    expect(canStartRecording(ready, { state: "idle" }, "zoom")).toBe(false);
  });
});

describe("canEnableMicrophone", () => {
  it("reflects microphoneAvailable from readiness", () => {
    expect(canEnableMicrophone(ready)).toBe(true);
    expect(canEnableMicrophone({ ...ready, microphoneAvailable: false })).toBe(false);
    expect(canEnableMicrophone(null)).toBe(false);
  });
});

describe("availableTracks", () => {
  const base: SessionMetadata = {
    schemaVersion: 2,
    sessionId: "s",
    startedAt: "2026-08-07T00:00:00.000Z",
    endedAt: "2026-08-07T00:05:00.000Z",
    durationSeconds: 300,
    micEnabled: false,
    recovered: false,
    interrupted: false,
    droppedMeetingFrames: 0,
    droppedMicFrames: 0,
    sampleRate: 48_000,
    meetingWav: "meeting.wav",
    microphoneWav: null,
    mixedWav: "mixed.wav",
    sourceApp: "teams",
    appBundleId: "com.blueear.app",
    appVersion: "0.1.0",
  };

  it("always includes mixed and meeting when present", () => {
    expect(availableTracks(base)).toEqual(["meeting", "mixed"]);
  });

  it("includes microphone only when recorded", () => {
    expect(
      availableTracks({
        ...base,
        micEnabled: true,
        microphoneWav: "microphone.wav",
      }),
    ).toEqual(["meeting", "microphone", "mixed"]);
  });
});
