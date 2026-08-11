import { useCallback, useEffect, useRef, useState } from "react";

import {
  dismissSession,
  getReadiness,
  getSessionState,
  listRecentSessions,
  onLevels,
  onSessionState,
  onSourceWarning,
  requestCaptureAccess,
  revealSession,
  startRecording,
  stopRecording,
} from "../app/recordingApi";
import {
  isAppError,
  type LevelsEvent,
  type MeetingAppId,
  type Readiness,
  type SessionMetadata,
  type SessionState,
} from "../types/recording";

export interface SourceWarning {
  code: string;
  receivedAtMs: number;
}

export function useRecordingSession() {
  const [readiness, setReadiness] = useState<Readiness | null>(null);
  const [sessionState, setSessionState] = useState<SessionState>({ state: "idle" });
  const [levels, setLevels] = useState<LevelsEvent>({ meeting: 0, microphone: 0 });
  const [warning, setWarning] = useState<SourceWarning | null>(null);
  const [recentSessions, setRecentSessions] = useState<SessionMetadata[]>([]);
  const [busyError, setBusyError] = useState<string | null>(null);
  const mountedRef = useRef(true);

  const refreshReadiness = useCallback(async () => {
    try {
      const next = await getReadiness();
      if (mountedRef.current) setReadiness(next);
    } catch {
      // Readiness polling failures are non-fatal; the next tick retries.
    }
  }, []);

  const refreshRecentSessions = useCallback(async () => {
    try {
      const sessions = await listRecentSessions(10);
      if (mountedRef.current) setRecentSessions(sessions);
    } catch {
      // Non-fatal: the result screen simply shows nothing extra.
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;

    void refreshReadiness();
    void refreshRecentSessions();
    void getSessionState().then((state) => {
      if (mountedRef.current) setSessionState(state);
    });

    const unlistenPromises = [
      onSessionState((state) => {
        if (!mountedRef.current) return;
        setSessionState(state);
        if (state.state === "completed" || state.state === "idle") {
          void refreshRecentSessions();
        }
      }),
      onLevels((next) => {
        if (mountedRef.current) setLevels(next);
      }),
      onSourceWarning((next) => {
        if (mountedRef.current) setWarning({ code: next.code, receivedAtMs: Date.now() });
      }),
    ];

    const readinessInterval = window.setInterval(() => void refreshReadiness(), 3000);

    return () => {
      mountedRef.current = false;
      window.clearInterval(readinessInterval);
      for (const p of unlistenPromises) {
        void p.then((unlisten) => unlisten());
      }
    };
  }, [refreshReadiness, refreshRecentSessions]);

  const probePermission = useCallback(async () => {
    setBusyError(null);
    try {
      const state = await requestCaptureAccess();
      setReadiness((prev) => (prev ? { ...prev, permissionState: state } : prev));
      return state;
    } catch (err) {
      setBusyError(describeError(err));
      return null;
    }
  }, []);

  const start = useCallback(async (sourceApp: MeetingAppId, includeMicrophone: boolean) => {
    setBusyError(null);
    try {
      await startRecording(sourceApp, includeMicrophone);
    } catch (err) {
      setBusyError(describeError(err));
    }
  }, []);

  const stop = useCallback(async (sessionId: string) => {
    setBusyError(null);
    try {
      await stopRecording(sessionId);
    } catch (err) {
      setBusyError(describeError(err));
    }
  }, []);

  const reveal = useCallback(async (sessionId: string) => {
    try {
      await revealSession(sessionId);
    } catch (err) {
      setBusyError(describeError(err));
    }
  }, []);

  const dismiss = useCallback(async () => {
    try {
      await dismissSession();
    } catch {
      // Non-fatal.
    }
  }, []);

  return {
    readiness,
    sessionState,
    levels,
    warning,
    recentSessions,
    busyError,
    probePermission,
    start,
    stop,
    reveal,
    dismiss,
    refreshReadiness,
    refreshRecentSessions,
  };
}

function describeError(err: unknown): string {
  if (isAppError(err)) return err.message;
  if (typeof err === "string") return err;
  return "Something went wrong. Please try again.";
}
