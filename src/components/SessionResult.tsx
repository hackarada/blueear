import { AlertTriangle, Check, FolderOpen } from "lucide-react";

import { formatDuration } from "../lib/format";
import type { SessionMetadata } from "../types/recording";
import type { TranscriptionOverview } from "../types/transcription";
import { TranscriptionPanel } from "./TranscriptionPanel";
import { Button } from "./ui/Button";

interface SessionResultProps {
  metadata: SessionMetadata;
  transcriptionOverview: TranscriptionOverview | null;
  onReveal: (sessionId: string) => void;
  onOpenTranscriptionSettings: () => void;
  onRecordAnother: () => void;
}

export function SessionResult({
  metadata,
  transcriptionOverview,
  onReveal,
  onOpenTranscriptionSettings,
  onRecordAnother,
}: SessionResultProps) {
  return (
    <div className="screen session-result">
      <div className="result-icon" aria-hidden="true">
        <Check size={24} strokeWidth={2.5} />
      </div>
      <h2>Recording saved</h2>
      <div className="result-duration">{formatDuration(metadata.durationSeconds)}</div>

      {metadata.recovered && (
        <div className="banner warning">
          <AlertTriangle size={14} strokeWidth={2} style={{ flexShrink: 0, marginTop: 2 }} />
          This recording was recovered after Blue Ear closed unexpectedly. Some audio near the
          end may be missing.
        </div>
      )}
      {!metadata.recovered && metadata.interrupted && (
        <div className="banner warning">
          <AlertTriangle size={14} strokeWidth={2} style={{ flexShrink: 0, marginTop: 2 }} />
          Recording stopped early because disk space ran low.
        </div>
      )}
      {(metadata.droppedMeetingFrames > 0 || metadata.droppedMicFrames > 0) && (
        <div className="banner warning">
          <AlertTriangle size={14} strokeWidth={2} style={{ flexShrink: 0, marginTop: 2 }} />
          A few audio frames were dropped under heavy load.
        </div>
      )}

      <ul className="result-tracks">
        {metadata.meetingWav && <li>{metadata.meetingWav}</li>}
        {metadata.micEnabled && metadata.microphoneWav && <li>microphone.wav</li>}
        <li>mixed.wav</li>
      </ul>

      <TranscriptionPanel
        sessionId={metadata.sessionId}
        overview={transcriptionOverview}
        onOpenSettings={onOpenTranscriptionSettings}
        defaultTranscriptOpen
      />

      <div className="controls stacked">
        <Button variant="secondary" fullWidth onClick={() => onReveal(metadata.sessionId)}>
          <FolderOpen size={16} />
          Reveal in folder
        </Button>
        <Button fullWidth onClick={onRecordAnother}>
          Record another
        </Button>
      </div>
    </div>
  );
}
