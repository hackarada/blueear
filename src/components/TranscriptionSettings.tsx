import { AlertTriangle, Check, Download, ExternalLink, Trash2 } from "lucide-react";

import { formatBundleSize, notReadyLabel, selectedProvider } from "../lib/transcriptionUi";
import type {
  ModelDownloadPage,
  ProviderStatus,
  TranscriptionOverview,
  TranscriptionPreferences,
} from "../types/transcription";
import { Button } from "./ui/Button";
import { Toggle } from "./ui/Toggle";

interface TranscriptionSettingsProps {
  overview: TranscriptionOverview | null;
  busy: boolean;
  error: string | null;
  onUpdatePreferences: (changes: Partial<TranscriptionPreferences>) => void;
  onImportBundle: () => void;
  onDeleteBundle: (bundleId: string) => void;
  onOpenModelPage: (page: ModelDownloadPage) => void;
}

interface ProviderCardProps {
  provider: ProviderStatus;
  selected: boolean;
  disabled: boolean;
  onSelect: () => void;
}

function ProviderCard({ provider, selected, disabled, onSelect }: ProviderCardProps) {
  const unreadyLabel = provider.ready ? "" : notReadyLabel(provider);

  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      disabled={disabled}
      className={`provider-card${selected ? " provider-card--selected" : ""}`}
      onClick={onSelect}
    >
      <div className="provider-card__header">
        <span className="provider-card__name">{provider.displayName}</span>
        {selected && <Check size={14} strokeWidth={2.5} aria-hidden="true" />}
      </div>
      <span className="provider-card__summary">{provider.summary}</span>
      <span className="provider-card__facts">
        <span>
          {provider.minimumMacos ? (
            <>
              macOS <b>{provider.minimumMacos}+</b>
            </>
          ) : null}
          {provider.minimumMacos && provider.minimumWindows ? " · " : null}
          {provider.minimumWindows ? (
            <>
              Windows <b>{provider.minimumWindows}+</b>
            </>
          ) : null}
          {!provider.minimumMacos && !provider.minimumWindows ? <b>Any</b> : null}
        </span>
        <span>
          Setup <b>{provider.requiresModelImport ? "Model import" : "None"}</b>
        </span>
        <span>
          Speaker labels <b>{provider.supportsRemoteSpeakerLabels ? "Yes" : "Track only"}</b>
        </span>
      </span>
      {provider.id !== "none" && unreadyLabel && (
        <span className="provider-card__status">{unreadyLabel}</span>
      )}
    </button>
  );
}

export function TranscriptionSettings({
  overview,
  busy,
  error,
  onUpdatePreferences,
  onImportBundle,
  onDeleteBundle,
  onOpenModelPage,
}: TranscriptionSettingsProps) {
  if (!overview) {
    return (
      <div className="screen transcription-settings">
        <h2>Transcription</h2>
        <p className="settings-hint">Loading...</p>
      </div>
    );
  }

  const { preferences, providers, installedBundles, anyProviderInstallable } = overview;
  const current = selectedProvider(overview);
  const importNeeded = providers.some((p) => p.requiresModelImport);
  const fluidSelected = preferences.provider === "fluidaudio";
  const whisperSelected = preferences.provider === "whisper";

  return (
    <div className="screen transcription-settings">
      <h2>Transcription</h2>
      <p className="settings-hint">
        Everything runs on this computer. Blue Ear never sends your audio anywhere, and
        transcription only starts when you ask for it.
      </p>

      {!anyProviderInstallable && (
        <div className="banner warning">
          <AlertTriangle size={14} strokeWidth={2} style={{ flexShrink: 0, marginTop: 2 }} />
          No transcription engine can run on this computer. Recordings are still saved as audio.
        </div>
      )}

      {error && <div className="banner error">{error}</div>}

      <div className="section-label">Provider</div>
      <div className="provider-grid" role="radiogroup" aria-label="Transcription provider">
        {providers.map((provider) => (
          <ProviderCard
            key={provider.id}
            provider={provider}
            selected={preferences.provider === provider.id}
            disabled={busy}
            onSelect={() => onUpdatePreferences({ provider: provider.id })}
          />
        ))}
      </div>

      {importNeeded && (
        <>
          <div className="section-label">Models</div>
          {installedBundles.length === 0 ? (
            <div className="model-setup">
              <p className="settings-hint">
                Providers that need models expect a Blue Ear model bundle. Blue Ear never
                downloads models itself — you get them in a browser, pack them, then import the
                folder.
              </p>
              <ol className="model-setup__steps">
                <li>
                  FluidAudio: download ASR and diarization folders, then run{" "}
                  <code>scripts/pack-fluidaudio-bundle.sh</code> for a{" "}
                  <code>fluidaudio-v1</code> bundle.
                </li>
                <li>
                  Whisper: place a ggml <code>.bin</code> under <code>ggml/</code> in a{" "}
                  <code>whisper-v1</code> bundle with a <code>manifest.json</code>.
                </li>
                <li>Import that bundle folder here.</li>
                <li>Select the provider above — importing alone does not turn it on.</li>
              </ol>
              <div className="model-setup__actions">
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={busy}
                  onClick={() => onOpenModelPage("asr")}
                >
                  <ExternalLink size={14} />
                  Open ASR models
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={busy}
                  onClick={() => onOpenModelPage("diarization")}
                >
                  <ExternalLink size={14} />
                  Open diarization models
                </Button>
              </div>
            </div>
          ) : (
            <>
              <ul className="bundle-list">
                {installedBundles.map((bundle) => (
                  <li key={bundle.bundleId} className="bundle-row">
                    <div>
                      <span className="bundle-row__name">{bundle.displayName}</span>
                      <span className="bundle-row__meta">{formatBundleSize(bundle.totalBytes)}</span>
                    </div>
                    <Button
                      variant="ghost"
                      size="sm"
                      disabled={busy}
                      onClick={() => onDeleteBundle(bundle.bundleId)}
                    >
                      <Trash2 size={14} />
                      Remove
                    </Button>
                  </li>
                ))}
              </ul>
              {!fluidSelected && !whisperSelected && (
                <p className="settings-hint">
                  Models are installed. Select FluidAudio or Whisper above when you want to use
                  them — importing does not change the provider.
                </p>
              )}
            </>
          )}
          <Button variant="secondary" disabled={busy} onClick={() => onImportBundle()}>
            <Download size={16} />
            Import model bundle
          </Button>
        </>
      )}

      <div className="section-label">Behaviour</div>
      <Toggle
        label="Transcribe automatically when a recording ends"
        checked={preferences.autoTranscribe}
        disabled={busy || preferences.provider === "none"}
        onChange={(checked) => onUpdatePreferences({ autoTranscribe: checked })}
      />
      <Toggle
        label="Label meeting participants as separate speakers"
        checked={preferences.diarizeRemoteSpeakers}
        disabled={busy || !current?.supportsRemoteSpeakerLabels}
        onChange={(checked) => onUpdatePreferences({ diarizeRemoteSpeakers: checked })}
      />
      {current && !current.supportsRemoteSpeakerLabels && current.id !== "none" && (
        <p className="settings-hint">
          {current.displayName} cannot tell participants apart, so meeting audio is labelled as one
          speaker.
        </p>
      )}
    </div>
  );
}
