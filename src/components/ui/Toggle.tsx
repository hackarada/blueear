interface ToggleProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: string;
  disabled?: boolean;
}

export function Toggle({ checked, onChange, label, disabled }: ToggleProps) {
  return (
    <label className={`toggle ${disabled ? "toggle--disabled" : ""}`}>
      <span className="toggle__label">{label}</span>
      <button
        type="button"
        role="switch"
        aria-checked={checked}
        aria-label={label}
        disabled={disabled}
        className={`toggle__track ${checked ? "toggle__track--on" : ""}`}
        onClick={() => {
          if (!disabled) onChange(!checked);
        }}
      >
        <span className="toggle__thumb" />
      </button>
    </label>
  );
}
