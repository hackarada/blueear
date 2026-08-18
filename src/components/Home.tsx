import { AudioLines, Captions, ChevronRight, Mic } from "lucide-react";

import { formatDuration, formatStartedAt } from "../lib/format";
import { canTranscribe, selectedProvider } from "../lib/transcriptionUi";
import type { SessionMetadata } from "../types/recording";
import type { TranscriptionOverview } from "../types/transcription";
import { BrandLogo } from "./ui/BrandLogo";
import { Button } from "./ui/Button";

interface HomeProps {
  lastSession: SessionMetadata | null;
  transcriptionOverview: TranscriptionOverview | null;
  meetingAppSummary: string;
  onStartRecording: () => void;
  onOpenLastSession: () => void;
  onTranscribe: () => void;
}

export function Home({
  lastSession,
  transcriptionOverview,
  meetingAppSummary,
  onStartRecording,
  onOpenLastSession,
  onTranscribe,
}: HomeProps) {
  const provider = selectedProvider(transcriptionOverview);
  const transcriptionReady = canTranscribe(transcriptionOverview);
  const transcriptionLabel =
    !provider || provider.id === "none"
      ? "Off — audio only"
      : provider.ready
        ? provider.displayName
        : `${provider.displayName} needs setup`;

  const transcribeLabel = transcriptionReady
    ? "Transcribe a recording"
    : "Set up transcription";

  return (
    <div className="screen home">
      <div className="home-hero">
        <BrandLogo size={40} />
        <div className="home-hero__text">
          <h1>Blue Ear</h1>
          <p>Local meeting recorder for Teams and Zoom</p>
        </div>
      </div>

      <div className="home-actions">
        <Button fullWidth onClick={onStartRecording}>
          <Mic size={16} />
          Start recording
        </Button>
        <Button
          variant="secondary"
          fullWidth
          disabled={!lastSession}
          onClick={onOpenLastSession}
        >
          <AudioLines size={16} />
          {lastSession ? "Open latest recording" : "No recordings yet"}
        </Button>
        <Button variant="secondary" fullWidth onClick={onTranscribe}>
          <Captions size={16} />
          {transcribeLabel}
        </Button>
      </div>

      {lastSession && (
        <button type="button" className="home-recent" onClick={onOpenLastSession}>
          <div className="home-recent__copy">
            <span className="section-label">Latest recording</span>
            <span className="home-recent__title">{formatStartedAt(lastSession.startedAt)}</span>
            <span className="home-recent__meta">
              {formatDuration(lastSession.durationSeconds)}
            </span>
          </div>
          <ChevronRight size={16} aria-hidden="true" />
        </button>
      )}

      <dl className="home-status">
        <div>
          <dt>Meeting apps</dt>
          <dd>{meetingAppSummary}</dd>
        </div>
        <div>
          <dt>Transcription</dt>
          <dd>{transcriptionLabel}</dd>
        </div>
      </dl>
    </div>
  );
}
