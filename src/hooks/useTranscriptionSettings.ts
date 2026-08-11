import { useCallback, useEffect, useRef, useState } from "react";

import {
  deleteModelBundle,
  getTranscriptionOverview,
  importModelBundle,
  setTranscriptionPreferences,
} from "../app/transcriptionApi";
import { describeError } from "../lib/errors";
import type { TranscriptionOverview, TranscriptionPreferences } from "../types/transcription";

// Every mutation returns a fresh overview from Rust rather than patching the
// local copy, because changing a preference can change readiness: selecting
// FluidAudio without models makes it unready in the same round trip.
export function useTranscriptionSettings() {
  const [overview, setOverview] = useState<TranscriptionOverview | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mountedRef = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const next = await getTranscriptionOverview();
      if (mountedRef.current) setOverview(next);
    } catch (err) {
      if (mountedRef.current) setError(describeError(err));
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    void refresh();
    return () => {
      mountedRef.current = false;
    };
  }, [refresh]);

  const run = useCallback(async (action: () => Promise<TranscriptionOverview | null>) => {
    setBusy(true);
    setError(null);
    try {
      const next = await action();
      if (next && mountedRef.current) setOverview(next);
    } catch (err) {
      if (mountedRef.current) setError(describeError(err));
    } finally {
      if (mountedRef.current) setBusy(false);
    }
  }, []);

  const updatePreferences = useCallback(
    (changes: Partial<TranscriptionPreferences>) => {
      if (!overview) return Promise.resolve();
      return run(() => setTranscriptionPreferences({ ...overview.preferences, ...changes }));
    },
    [overview, run],
  );

  const importBundle = useCallback(() => run(importModelBundle), [run]);

  const deleteBundle = useCallback(
    (bundleId: string) => run(() => deleteModelBundle(bundleId)),
    [run],
  );

  return { overview, busy, error, refresh, updatePreferences, importBundle, deleteBundle };
}
