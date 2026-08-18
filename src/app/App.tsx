import { useEffect, useMemo, useState } from "react";
import { X } from "lucide-react";

import { Home } from "../components/Home";
import { Onboarding } from "../components/Onboarding";
import { Recorder } from "../components/Recorder";
import { Recordings } from "../components/Recordings";
import { SessionResult } from "../components/SessionResult";
import { Sidebar, type AppView } from "../components/Sidebar";
import { TranscriptionSettings } from "../components/TranscriptionSettings";
import { Button } from "../components/ui/Button";
import { ThemeSelector } from "../components/ui/ThemeSelector";
import { useRecordingSession } from "../hooks/useRecordingSession";
import { useTranscriptionSettings } from "../hooks/useTranscriptionSettings";
import { meetingAppSummary } from "../lib/recordingUi";
import { canTranscribe } from "../lib/transcriptionUi";
import { warningLabelFor } from "../lib/warnings";
import { onNavigate } from "./recordingApi";
import { openModelDownloadPage } from "./transcriptionApi";

const ONBOARDING_KEY = "blueear.onboarding.acknowledged";

export default function App() {
  const [onboarded, setOnboarded] = useState(() => localStorage.getItem(ONBOARDING_KEY) === "1");
  const [probing, setProbing] = useState(false);
  const [view, setView] = useState<AppView>("home");

  const {
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
  } = useRecordingSession();

  // Owned here rather than inside the settings screen so the result screen can
  // tell whether transcription is configured without a second round trip.
  const transcription = useTranscriptionSettings();

  const [visibleWarning, setVisibleWarning] = useState<string | null>(null);
  useEffect(() => {
    if (!warning) return;
    setVisibleWarning(warningLabelFor(warning.code));
    const id = window.setTimeout(() => setVisibleWarning(null), 6000);
    return () => window.clearTimeout(id);
  }, [warning]);

  useEffect(() => {
    const unlistenPromise = onNavigate((screen) => {
      if (screen === "recordings") setView("recordings");
    });
    return () => void unlistenPromise.then((unlisten) => unlisten());
  }, []);

  const handleContinueOnboarding = () => {
    localStorage.setItem(ONBOARDING_KEY, "1");
    setOnboarded(true);
  };

  const handleProbePermission = async () => {
    setProbing(true);
    await probePermission();
    setProbing(false);
  };

  const recordingActive =
    sessionState.state === "preparing" ||
    sessionState.state === "recording" ||
    sessionState.state === "recovering" ||
    sessionState.state === "stopping" ||
    sessionState.state === "finalizing";

  const sessionScreen = useMemo(() => {
    if (sessionState.state === "completed") {
      return (
        <SessionResult
          metadata={sessionState.metadata}
          transcriptionOverview={transcription.overview}
          onReveal={reveal}
          onOpenTranscriptionSettings={() => setView("transcription")}
          onRecordAnother={() => {
            void dismiss();
            void refreshReadiness();
            setView("recorder");
          }}
        />
      );
    }

    if (sessionState.state === "failed") {
      return (
        <div className="screen error-screen">
          <div className="result-icon error" aria-hidden="true">
            <X size={24} strokeWidth={2.5} />
          </div>
          <h2>Recording couldn't start</h2>
          <p className="error-message">{sessionState.error.message}</p>
          <Button
            fullWidth
            onClick={() => {
              void dismiss();
              void refreshReadiness();
              setView("recorder");
            }}
          >
            Try again
          </Button>
        </div>
      );
    }

    return (
      <Recorder
        readiness={readiness}
        sessionState={sessionState}
        levels={levels}
        warningLabel={visibleWarning}
        busyError={busyError}
        probing={probing}
        onProbePermission={handleProbePermission}
        onStart={start}
        onStop={stop}
      />
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sessionState, readiness, levels, visibleWarning, busyError, probing, transcription.overview]);

  if (!onboarded) {
    return (
      <>
        <div className="onboarding-theme">
          <ThemeSelector />
        </div>
        <Onboarding
          onContinue={handleContinueOnboarding}
          platform={readiness?.platform}
          recordingsPathDisplay={readiness?.recordingsPathDisplay}
        />
      </>
    );
  }

  const lastSession = recentSessions[0] ?? null;
  const homeShowsSession = view === "home" && recordingActive;

  let main = null;
  switch (view) {
    case "home":
      main = homeShowsSession ? (
        sessionScreen
      ) : (
        <Home
          lastSession={lastSession}
          transcriptionOverview={transcription.overview}
          meetingAppSummary={meetingAppSummary(readiness?.meetingApps)}
          onStartRecording={() => {
            if (sessionState.state === "completed" || sessionState.state === "failed") {
              void dismiss();
            }
            setView("recorder");
          }}
          onOpenLastSession={() => setView("recordings")}
          onTranscribe={() =>
            setView(canTranscribe(transcription.overview) && lastSession ? "recordings" : "transcription")
          }
        />
      );
      break;
    case "recorder":
      main = sessionScreen;
      break;
    case "recordings":
      main = (
        <Recordings
          sessions={recentSessions}
          transcriptionOverview={transcription.overview}
          onReveal={reveal}
          onRefresh={refreshRecentSessions}
          onOpenTranscriptionSettings={() => setView("transcription")}
        />
      );
      break;
    case "transcription":
      main = (
        <TranscriptionSettings
          overview={transcription.overview}
          busy={transcription.busy}
          error={transcription.error}
          onUpdatePreferences={(changes) => void transcription.updatePreferences(changes)}
          onImportBundle={() => void transcription.importBundle()}
          onDeleteBundle={(bundleId) => void transcription.deleteBundle(bundleId)}
          onOpenModelPage={(page) => void openModelDownloadPage(page)}
        />
      );
      break;
    default: {
      const _never: never = view;
      main = _never;
    }
  }

  return (
    <div className="app-shell">
      <Sidebar view={view} recordingActive={recordingActive} onNavigate={setView} />
      <div className="app-main">{main}</div>
    </div>
  );
}
