import { FileText, RotateCcw, Type, X } from "lucide-react";

import { useTranscriptionJob } from "../hooks/useTranscriptionJob";
import {
  canTranscribe,
  formatTimecode,
  isJobResumable,
  isJobRunning,
  jobStatusLabel,
  selectedProvider,
  speakerLabel,
  TRANSCRIPT_TRACK_LABELS,
} from "../lib/transcriptionUi";
import type { TranscriptionOverview } from "../types/transcription";
import { Button } from "./ui/Button";

interface TranscriptionPanelProps {
  sessionId: string;
  overview: TranscriptionOverview | null;
  onOpenSettings: () => void;
}

export function TranscriptionPanel({
  sessionId,
  overview,
  onOpenSettings,
}: TranscriptionPanelProps) {
  const { job, transcript, error, start, retry, cancel, exportAs } = useTranscriptionJob(sessionId);
  const running = isJobRunning(job);
  const provider = selectedProvider(overview);

  // With no provider configured this is a recorder, and saying so beats an
  // action that can only fail. Once one is chosen but not ready, the settings
  // screen is where the problem can actually be fixed.
  if (!canTranscribe(overview) && !job && !transcript) {
    return (
      <div className="transcription-panel">
        <div className="transcription-panel__header">
          <span className="section-label">Transcript</span>
        </div>
        <p className="settings-hint">
          {provider && provider.id !== "none"
            ? `${provider.displayName} isn't ready yet.`
            : "Transcription is off. This recording is saved as audio only."}
        </p>
        <Button variant="ghost" size="sm" onClick={onOpenSettings}>
          Transcription settings
        </Button>
      </div>
    );
  }

  return (
    <div className="transcription-panel">
      <div className="transcription-panel__header">
        <span className="section-label">Transcript</span>
        {job && <span className="transcription-panel__status">{jobStatusLabel(job.status)}</span>}
      </div>

      {running && job && (
        <>
          <div
            className="progress-bar"
            role="progressbar"
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(job.progress * 100)}
          >
            <div className="progress-bar__fill" style={{ width: `${job.progress * 100}%` }} />
          </div>
          <ul className="track-progress">
            {job.tracks.map((track) => (
              <li key={track.track}>
                <span>{TRANSCRIPT_TRACK_LABELS[track.track]}</span>
                <span>{jobStatusLabel(track.status)}</span>
              </li>
            ))}
          </ul>
        </>
      )}

      {job?.error && !running && <div className="banner error">{job.error.message}</div>}
      {error && <div className="banner error">{error}</div>}

      {transcript && !running && (
        <div className="transcript-view">
          {transcript.segments.map((segment, index) => (
            <p key={`${segment.startSeconds}-${index}`} className="transcript-segment">
              <span className="transcript-segment__time">
                {formatTimecode(segment.startSeconds)}
              </span>
              <span className="transcript-segment__speaker">{speakerLabel(segment.speaker)}</span>
              <span className="transcript-segment__text">{segment.text}</span>
            </p>
          ))}
        </div>
      )}

      <div className="controls">
        {running ? (
          <Button variant="secondary" size="sm" onClick={() => void cancel()}>
            <X size={14} />
            Cancel
          </Button>
        ) : (
          <>
            {isJobResumable(job) && (
              <Button variant="secondary" size="sm" onClick={() => void retry()}>
                <RotateCcw size={14} />
                Retry
              </Button>
            )}
            {!job && (
              <Button size="sm" onClick={() => void start()}>
                <FileText size={14} />
                Transcribe
              </Button>
            )}
            {transcript && (
              <>
                <Button variant="secondary" size="sm" onClick={() => void exportAs("text")}>
                  <Type size={14} />
                  Save as text
                </Button>
                <Button variant="secondary" size="sm" onClick={() => void exportAs("vtt")}>
                  <FileText size={14} />
                  Save as VTT
                </Button>
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}
