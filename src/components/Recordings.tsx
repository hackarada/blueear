import { useEffect, useState } from "react";
import { convertFileSrc } from "@tauri-apps/api/core";
import { FolderOpen, Mic } from "lucide-react";

import { getSessionAssetPath } from "../app/recordingApi";
import { formatDuration, formatStartedAt } from "../lib/format";
import { availableTracks } from "../lib/recordingUi";
import { isAppError, type RecordingTrack, type SessionMetadata } from "../types/recording";
import type { TranscriptionOverview } from "../types/transcription";
import { TranscriptionPanel } from "./TranscriptionPanel";
import { Button } from "./ui/Button";

interface RecordingsProps {
  sessions: SessionMetadata[];
  transcriptionOverview: TranscriptionOverview | null;
  onReveal: (sessionId: string) => void;
  onRefresh: () => void;
  onOpenTranscriptionSettings: () => void;
}

const TRACK_LABELS: Record<RecordingTrack, string> = {
  meeting: "Meeting",
  teams: "Meeting",
  microphone: "Microphone",
  mixed: "Mixed",
};

function RecordingCard({
  session,
  transcriptionOverview,
  onReveal,
  onOpenTranscriptionSettings,
}: {
  session: SessionMetadata;
  transcriptionOverview: TranscriptionOverview | null;
  onReveal: (sessionId: string) => void;
  onOpenTranscriptionSettings: () => void;
}) {
  const tracks = availableTracks(session);
  const [activeTrack, setActiveTrack] = useState<RecordingTrack | null>(null);
  const [assetUrl, setAssetUrl] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const selectTrack = async (track: RecordingTrack) => {
    if (track === activeTrack) return;
    setActiveTrack(track);
    setAssetUrl(null);
    setError(null);
    setLoading(true);
    try {
      const path = await getSessionAssetPath(session.sessionId, track);
      setAssetUrl(convertFileSrc(path));
    } catch (err) {
      setError(isAppError(err) ? err.message : "Couldn't load that track.");
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className="recording-card">
      <div className="recording-card-header">
        <span className="recording-date">{formatStartedAt(session.startedAt)}</span>
        <span className="recording-duration">{formatDuration(session.durationSeconds)}</span>
      </div>

      <div className="track-picker">
        {tracks.map((track) => (
          <button
            key={track}
            className={`track-chip ${activeTrack === track ? "active" : ""}`}
            onClick={() => void selectTrack(track)}
          >
            {TRACK_LABELS[track]}
          </button>
        ))}
      </div>

      {loading && <div className="recording-player-status">Loading...</div>}
      {error && <div className="recording-player-status">{error}</div>}
      {assetUrl && !loading && !error && (
        <audio className="recording-player" controls preload="none" src={assetUrl} />
      )}

      <TranscriptionPanel
        sessionId={session.sessionId}
        overview={transcriptionOverview}
        onOpenSettings={onOpenTranscriptionSettings}
      />

      <div className="recording-card-actions">
        <Button variant="secondary" size="sm" onClick={() => onReveal(session.sessionId)}>
          <FolderOpen size={14} />
          Reveal in folder
        </Button>
      </div>
    </div>
  );
}

export function Recordings({
  sessions,
  transcriptionOverview,
  onReveal,
  onRefresh,
  onOpenTranscriptionSettings,
}: RecordingsProps) {
  useEffect(() => {
    void onRefresh();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="screen recordings">
      <div className="recordings-header">
        <h2>Recordings</h2>
      </div>

      {sessions.length === 0 ? (
        <div className="recordings-empty">
          <Mic size={24} strokeWidth={1.5} />
          <span>No recordings yet</span>
        </div>
      ) : (
        <div className="recordings-list">
          {sessions.map((session) => (
            <RecordingCard
              key={session.sessionId}
              session={session}
              transcriptionOverview={transcriptionOverview}
              onReveal={onReveal}
              onOpenTranscriptionSettings={onOpenTranscriptionSettings}
            />
          ))}
        </div>
      )}
    </div>
  );
}
