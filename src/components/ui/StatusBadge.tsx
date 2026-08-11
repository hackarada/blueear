import { Check, Circle, X } from "lucide-react";

type StatusState = "ok" | "bad" | "pending";

interface StatusBadgeProps {
  state: StatusState;
}

export function StatusBadge({ state }: StatusBadgeProps) {
  const Icon = state === "ok" ? Check : state === "bad" ? X : Circle;
  return (
    <span className={`status-badge status-badge--${state}`} aria-hidden="true">
      <Icon size={12} strokeWidth={2.5} />
    </span>
  );
}
