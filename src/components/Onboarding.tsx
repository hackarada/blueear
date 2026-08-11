import { Button } from "./ui/Button";
import { BrandLogo } from "./ui/BrandLogo";

interface OnboardingProps {
  onContinue: () => void;
  recordingsPathDisplay?: string;
  platform?: string;
}

export function Onboarding({
  onContinue,
  recordingsPathDisplay = "~/Music/BlueEar/Recordings",
  platform = "macos",
}: OnboardingProps) {
  const deviceWord = platform === "windows" ? "PC" : "Mac";
  const permissionCopy =
    platform === "windows"
      ? "Windows may ask for Microphone access if you enable that track. Meeting audio uses process loopback and does not require a separate system-audio permission."
      : "macOS will ask for System Audio Recording access (and Microphone access if you enable that track).";

  return (
    <div className="screen onboarding">
      <div className="brand">
        <BrandLogo size={44} />
        <h1>Blue Ear</h1>
        <p className="subtitle">Local recordings of your Teams and Zoom meetings</p>
      </div>

      <ul className="onboarding-points">
        <li>
          <strong>Local only.</strong> Recordings are saved to{" "}
          <code>{recordingsPathDisplay}</code> on this {deviceWord}. Nothing is uploaded
          anywhere.
        </li>
        <li>
          <strong>Desktop meeting apps only.</strong> Blue Ear isolates audio from the native
          Microsoft Teams or Zoom desktop app (not a browser tab or PWA).
        </li>
        <li>
          <strong>You're responsible for consent.</strong> Blue Ear cannot verify that meeting
          participants have agreed to be recorded. Follow your organization's policy and
          applicable law before recording.
        </li>
        <li>
          <strong>Permissions.</strong> {permissionCopy}
        </li>
      </ul>

      <Button fullWidth onClick={onContinue}>
        I understand, continue
      </Button>
    </div>
  );
}
