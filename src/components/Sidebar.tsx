import { AudioLines, Captions, Home } from "lucide-react";

import { BrandLogo } from "./ui/BrandLogo";
import { ThemeSelector } from "./ui/ThemeSelector";

export type AppView = "home" | "recorder" | "recordings" | "transcription";

interface SidebarProps {
  view: AppView;
  recordingActive: boolean;
  onNavigate: (view: AppView) => void;
}

const ITEMS: { id: "home" | "recordings" | "transcription"; label: string; icon: typeof Home }[] = [
  { id: "home", label: "Home", icon: Home },
  { id: "recordings", label: "Recordings", icon: AudioLines },
  { id: "transcription", label: "Transcription", icon: Captions },
];

function navCurrent(view: AppView): "home" | "recordings" | "transcription" {
  switch (view) {
    case "home":
    case "recorder":
      return "home";
    case "recordings":
      return "recordings";
    case "transcription":
      return "transcription";
    default: {
      const _never: never = view;
      return _never;
    }
  }
}

export function Sidebar({ view, recordingActive, onNavigate }: SidebarProps) {
  const current = navCurrent(view);

  return (
    <nav className="app-sidebar" aria-label="Main">
      <div className="app-sidebar__brand">
        <BrandLogo size={28} />
        <span className="app-sidebar__name">Blue Ear</span>
      </div>

      <ul className="app-sidebar__nav">
        {ITEMS.map(({ id, label, icon: Icon }) => {
          const selected = current === id;
          return (
            <li key={id}>
              <button
                type="button"
                className={`app-sidebar__item${selected ? " app-sidebar__item--current" : ""}`}
                aria-current={selected ? "page" : undefined}
                onClick={() => onNavigate(id === "home" && recordingActive ? "recorder" : id)}
              >
                <Icon size={16} strokeWidth={2} aria-hidden="true" />
                <span>{label}</span>
                {id === "home" && recordingActive && (
                  <span className="app-sidebar__live" aria-label="Recording in progress" />
                )}
              </button>
            </li>
          );
        })}
      </ul>

      <div className="app-sidebar__footer">
        <ThemeSelector />
      </div>
    </nav>
  );
}
