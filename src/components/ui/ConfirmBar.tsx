import { Button } from "./Button";

interface ConfirmBarProps {
  message: string;
  confirmLabel: string;
  onConfirm: () => void;
  onCancel: () => void;
  busy?: boolean;
}

export function ConfirmBar({
  message,
  confirmLabel,
  onConfirm,
  onCancel,
  busy = false,
}: ConfirmBarProps) {
  return (
    <div className="confirm-bar" role="group" aria-label="Confirm save">
      <p className="confirm-bar__message">{message}</p>
      <div className="confirm-bar__actions">
        <Button size="sm" disabled={busy} onClick={onConfirm}>
          {confirmLabel}
        </Button>
        <Button variant="ghost" size="sm" disabled={busy} onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </div>
  );
}
