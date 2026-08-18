import { useCallback, useEffect, useRef, useState } from "react";

import {
  cancelTranscription,
  exportTranscript,
  getTranscript,
  getTranscriptionJob,
  onTranscriptionJob,
  retryTranscription,
  startTranscription,
} from "../app/transcriptionApi";
import { describeError } from "../lib/errors";
import type { ExportFormat, Job, Transcript } from "../types/transcription";

// Tracks one session's transcription. Job updates arrive as events while a job
// runs; the initial read covers the far more common case of opening a session
// that was transcribed on an earlier launch, or never at all.
export function useTranscriptionJob(sessionId: string) {
  const [job, setJob] = useState<Job | null>(null);
  const [transcript, setTranscript] = useState<Transcript | null>(null);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);

  const loadTranscript = useCallback(async () => {
    try {
      const next = await getTranscript(sessionId);
      if (mountedRef.current) setTranscript(next);
    } catch {
      // A missing transcript is the normal state before a job has ever run.
    }
  }, [sessionId]);

  useEffect(() => {
    mountedRef.current = true;
    setJob(null);
    setTranscript(null);

    void getTranscriptionJob(sessionId).then((next) => {
      if (!mountedRef.current) return;
      setJob(next);
      if (next?.tracks.some((track) => track.status === "completed")) void loadTranscript();
    });

    const unlistenPromise = onTranscriptionJob((next) => {
      if (!mountedRef.current || next.sessionId !== sessionId) return;
      setJob(next);
      // A partially successful job still has a transcript worth showing, so
      // reload on any terminal state rather than only on success.
      if (next.status === "completed" || next.status === "failed" || next.status === "cancelled") {
        void loadTranscript();
      }
    });

    return () => {
      mountedRef.current = false;
      void unlistenPromise.then((unlisten) => unlisten());
    };
  }, [sessionId, loadTranscript]);

  const run = useCallback(async (action: () => Promise<unknown>) => {
    setError(null);
    try {
      await action();
    } catch (err) {
      if (mountedRef.current) setError(describeError(err));
    }
  }, []);

  const start = useCallback(
    () => run(() => startTranscription(sessionId).then(setJob)),
    [run, sessionId],
  );

  const retry = useCallback(
    () => run(() => retryTranscription(sessionId).then(setJob)),
    [run, sessionId],
  );

  const cancel = useCallback(() => run(() => cancelTranscription(sessionId)), [run, sessionId]);

  const exportAs = useCallback(
    async (format: ExportFormat) => {
      setError(null);
      try {
        await exportTranscript(sessionId, format);
        return true;
      } catch (err) {
        if (mountedRef.current) setError(describeError(err));
        return false;
      }
    },
    [sessionId],
  );

  return { job, transcript, error, start, retry, cancel, exportAs };
}
