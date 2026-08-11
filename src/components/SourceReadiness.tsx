import type { ReactNode } from "react";
import { Loader2 } from "lucide-react";

import type { MeetingAppId, Readiness } from "../types/recording";
import { Button } from "./ui/Button";
import { StatusBadge } from "./ui/StatusBadge";

interface SourceReadinessProps {
  readiness: Readiness | null;
  selectedApp: MeetingAppId | null;
  onSelectApp: (id: MeetingAppId) => void;
  onRequestAccess: () => void;
  probing: boolean;
}

function StatusRow({
  label,
  detail,
  ok,
  action,
}: {
  label: string;
  detail: string;
  ok: boolean | "pending";
  action?: ReactNode;
}) {
  const state = ok === "pending" ? "pending" : ok ? "ok" : "bad";
  return (
    <div className="readiness-row">
      <StatusBadge state={state} />
      <div className="readiness-text">
        <span className="readiness-label">{label}</span>
        <span className="readiness-detail">{detail}</span>
      </div>
      {action}
    </div>
  );
}

export function SourceReadiness({
  readiness,
  selectedApp,
  onSelectApp,
  onRequestAccess,
  probing,
}: SourceReadinessProps) {
  if (!readiness) {
    return (
      <div className="readiness-loading">
        <Loader2 size={14} className="spin" />
        Checking your system...
      </div>
    );
  }

  const isWindows = readiness.platform === "windows";
  const permissionOk = readiness.permissionState === "granted";
  const permissionPending =
    readiness.permissionState === "unknown" || readiness.permissionState === "needsProbe";

  const osLabel = isWindows ? "Windows 10 (build 20348)+" : "macOS 14.4+";
  const osDetail = readiness.osSupported
    ? "Supported"
    : isWindows
      ? "Blue Ear requires Windows 10 build 20348 or later"
      : "Blue Ear requires macOS 14.4 or later";
  const installHint = isWindows
    ? "Not found in the usual install locations"
    : "Not found in /Applications";
  const deviceWord = isWindows ? "PC" : "Mac";
  const permissionLabel = isWindows ? "Capture access" : "System audio access";
  const permissionDenied = isWindows
    ? "Denied — check Windows privacy settings for Blue Ear"
    : "Denied — open System Settings to allow Blue Ear";

  return (
    <div>
      <p className="section-label">Readiness</p>
      <div className="readiness-list">
        <StatusRow label={osLabel} detail={osDetail} ok={readiness.osSupported} />
        {readiness.meetingApps.map((app) => (
          <StatusRow
            key={app.id}
            label={app.displayName}
            detail={
              !app.installed
                ? installHint
                : app.running
                  ? "Running"
                  : "Installed — open the app to enable recording"
            }
            ok={app.installed && app.running}
            action={
              app.installed ? (
                <Button
                  variant={selectedApp === app.id ? "primary" : "secondary"}
                  size="sm"
                  onClick={() => onSelectApp(app.id)}
                >
                  {selectedApp === app.id ? "Selected" : "Select"}
                </Button>
              ) : undefined
            }
          />
        ))}
        <StatusRow
          label="Microphone input"
          detail={
            readiness.microphoneAvailable
              ? `Available on this ${deviceWord}`
              : "No input device detected — mic recording is unavailable"
          }
          ok={readiness.microphoneAvailable}
        />
        <StatusRow
          label={permissionLabel}
          detail={
            permissionOk
              ? "Granted"
              : readiness.permissionState === "denied"
                ? permissionDenied
                : "Not yet checked"
          }
          ok={permissionOk ? true : permissionPending ? "pending" : false}
          action={
            !permissionOk && (
              <Button
                variant="secondary"
                size="sm"
                onClick={onRequestAccess}
                disabled={probing}
              >
                {probing ? "Checking..." : "Check access"}
              </Button>
            )
          }
        />
      </div>
    </div>
  );
}
