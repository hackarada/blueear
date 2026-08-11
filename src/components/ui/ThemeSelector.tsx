import { Monitor, Moon, Sun } from "lucide-react";

import { useTheme } from "../../hooks/useTheme";
import type { ThemePreference } from "../../lib/theme";

const OPTIONS: { value: ThemePreference; label: string; icon: typeof Sun }[] = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
];

export function ThemeSelector() {
  const { preference, setPreference } = useTheme();

  return (
    <div className="theme-selector" role="radiogroup" aria-label="Appearance">
      {OPTIONS.map(({ value, label, icon: Icon }) => {
        const selected = preference === value;
        return (
          <button
            key={value}
            type="button"
            role="radio"
            aria-checked={selected}
            aria-label={label}
            title={label}
            className={`theme-selector__option${selected ? " theme-selector__option--active" : ""}`}
            onClick={() => setPreference(value)}
          >
            <Icon size={14} strokeWidth={2} aria-hidden="true" />
          </button>
        );
      })}
    </div>
  );
}
