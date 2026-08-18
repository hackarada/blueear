import { useEffect, useState } from "react";
import { AlertTriangle } from "lucide-react";

import { formatElapsed } from "../lib/format";
import {
  autoSelectSourceApp,
  canEnableMicrophone,
  canStartRecording,
} from "../lib/recordingUi";
import {
  displayNameForApp,
  type LevelsEvent,
  type MeetingAppId,
  type Readiness,
  type SessionState,
} from "../types/recording";
import { SourceReadiness } from "./SourceReadiness";
import { Button } from "./ui/Button";
import { Toggle } from "./ui/Toggle";

interface RecorderProps {
  readiness: Readiness | null;
  sessionState: SessionState;
  levels: LevelsEvent;
  warningLabel: string | null;
  busyError: string | null;
  probing: boolean;
  onProbePermission: () => void;
  onStart: (sourceApp: MeetingAppId, includeMicrophone: boolean) => void;
  onStop: (sessionId: string) => void;
}

function LevelMeter({ label, value }: { label: string; value: number }) {
  const pct = Math.min(100, Math.round(Math.sqrt(Math.max(0, value)) * 100));
  return (
    <div className="level-meter">
      <span className="level-label">{label}</span>
      <div className="level-track">
        <div className="level-fill" style={{ width: `${pct}%` }} />
      </div>
    </div>
  );
}

export function Recorder({
  readiness,
  sessionState,
  levels,
  warningLabel,
  busyError,
  probing,
  onProbePermission,
  onStart,
  onStop,
}: RecorderProps) {
  const [includeMicrophone, setIncludeMicrophone] = useState(false);
  const [sourceApp, setSourceApp] = useState<MeetingAppId | null>(null);
  const [, setTick] = useState(0);

  useEffect(() => {
    const auto = autoSelectSourceApp(readiness);
    if (auto) {
      setSourceApp((prev) => prev ?? auto);
    }
  }, [readiness]);

  useEffect(() => {
    if (sessionState.state !== "recording" && sessionState.state !== "recovering") return;
    const id = window.setInterval(() => setTick((t) => t + 1), 250);
    return () => window.clearInterval(id);
  }, [sessionState.state]);

  const isTransitioning =
    sessionState.state === "preparing" ||
    sessionState.state === "stopping" ||
    sessionState.state === "finalizing";
  const isActive =
    sessionState.state === "recording" ||
    sessionState.state === "recovering";

  const canStart = canStartRecording(readiness, sessionState, sourceApp);
  const micToggleEnabled = canEnableMicrophone(readiness);

  let elapsedLabel = "00:00";
  if (sessionState.state === "recording" || sessionState.state === "recovering") {
    elapsedLabel = formatElapsed(Date.now() - sessionState.startedAtMs);
  }

  const isRecovering = sessionState.state === "recovering";
  const activeAppLabel =
    sessionState.state === "recording" || sessionState.state === "recovering"
      ? displayNameForApp(sessionState.sourceApp)
      : sourceApp
        ? displayNameForApp(sourceApp)
        : "Meeting";

  return (
    <div className="screen recorder">
      <div className="screen-header">
        <h2>Record</h2>
        <p>Isolate Teams or Zoom audio on this computer.</p>
      </div>

      {!isActive && !isTransitioning && (
        <SourceReadiness
          readiness={readiness}
          selectedApp={sourceApp}
          onSelectApp={setSourceApp}
          onRequestAccess={onProbePermission}
          probing={probing}
        />
      )}

      {isActive && (
        <div className="recording-status">
          <div className="recording-indicator">
            <span
              className={`recording-dot ${isRecovering ? "recording-dot--recovering" : "recording-dot--active"}`}
            />
            <span className="recording-label">
              {isRecovering ? "Reconnecting" : "Recording"}
            </span>
          </div>
          <span className="elapsed">{elapsedLabel}</span>
          {isRecovering && (
            <span className="recovering-label">Reconnecting to meeting audio...</span>
          )}
        </div>
      )}

      {isActive && (
        <div className="levels">
          <LevelMeter label={activeAppLabel} value={levels.meeting} />
          {(sessionState.state === "recording" || sessionState.state === "recovering") &&
            sessionState.micEnabled && <LevelMeter label="Mic" value={levels.microphone} />}
        </div>
      )}

      {warningLabel && (
        <div className="banner warning">
          <AlertTriangle size={14} strokeWidth={2} style={{ flexShrink: 0, marginTop: 2 }} />
          {warningLabel}
        </div>
      )}
      {busyError && (
        <div className="banner error">
          <AlertTriangle size={14} strokeWidth={2} style={{ flexShrink: 0, marginTop: 2 }} />
          {busyError}
        </div>
      )}

      {!isActive && !isTransitioning && (
        <Toggle
          checked={includeMicrophone}
          onChange={setIncludeMicrophone}
          label="Also record my microphone"
          disabled={!micToggleEnabled}
        />
      )}

      <div className="controls">
        {isActive || sessionState.state === "stopping" || sessionState.state === "finalizing" ? (
          <Button
            variant="danger"
            fullWidth
            disabled={sessionState.state === "stopping" || sessionState.state === "finalizing"}
            onClick={() => {
              if (sessionState.state === "recording" || sessionState.state === "recovering") {
                onStop(sessionState.sessionId);
              }
            }}
          >
            {sessionState.state === "stopping" || sessionState.state === "finalizing"
              ? "Stopping..."
              : "Stop recording"}
          </Button>
        ) : (
          <Button
            fullWidth
            disabled={!canStart || !sourceApp}
            onClick={() => {
              if (sourceApp) onStart(sourceApp, includeMicrophone);
            }}
          >
            {sessionState.state === "preparing" ? "Starting..." : "Start recording"}
          </Button>
        )}
      </div>
    </div>
  );
}
