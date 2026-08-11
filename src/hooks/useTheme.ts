import { useEffect, useState } from "react";

import {
  applyTheme,
  getStoredPreference,
  resolveTheme,
  setStoredPreference,
  type ResolvedTheme,
  type ThemePreference,
} from "../lib/theme";

export function useTheme() {
  const [preference, setPreferenceState] = useState<ThemePreference>(() => getStoredPreference());
  const [resolved, setResolved] = useState<ResolvedTheme>(() => resolveTheme(getStoredPreference()));

  useEffect(() => {
    const next = resolveTheme(preference);
    applyTheme(next);
    setResolved(next);

    if (preference !== "system") return;

    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      const systemResolved = resolveTheme("system");
      applyTheme(systemResolved);
      setResolved(systemResolved);
    };

    media.addEventListener("change", onChange);
    return () => media.removeEventListener("change", onChange);
  }, [preference]);

  const setPreference = (next: ThemePreference) => {
    setStoredPreference(next);
    setPreferenceState(next);
  };

  return { preference, resolved, setPreference };
}
