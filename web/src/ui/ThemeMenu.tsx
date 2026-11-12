import { useState } from "react";
import { setThemePreference, THEME_PREFERENCES, type ThemePreference, useThemePreference } from "../lib/theme";
import { Icon, type IconName } from "./Icon";
import { Popover } from "./Popover";

const OPTIONS: Record<ThemePreference, { text: string; icon: IconName }> = {
  system: { text: "System", icon: "system" },
  light: { text: "Light", icon: "sun" },
  dark: { text: "Dark", icon: "moon" },
};

export function ThemeMenu() {
  const preference = useThemePreference();
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);

  return (
    <>
      <button
        type="button"
        className="btn btn-ghost btn-icon btn-sm"
        aria-label={`Theme: ${OPTIONS[preference].text}`}
        aria-haspopup="dialog"
        aria-expanded={anchor !== null}
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
      >
        <Icon name={OPTIONS[preference].icon} />
      </button>
      {anchor && (
        <Popover anchor={anchor} label="Theme" width={168} onClose={() => setAnchor(null)}>
          <div className="menu" role="radiogroup" aria-label="Theme">
            {THEME_PREFERENCES.map((option) => (
              <button
                key={option}
                type="button"
                role="radio"
                className="menu-item"
                aria-checked={preference === option}
                onClick={() => {
                  setThemePreference(option);
                  setAnchor(null);
                  anchor.focus();
                }}
              >
                <Icon name={OPTIONS[option].icon} />
                {OPTIONS[option].text}
              </button>
            ))}
          </div>
        </Popover>
      )}
    </>
  );
}
