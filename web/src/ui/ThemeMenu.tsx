import { setThemePreference, THEME_PREFERENCES, type ThemePreference, useThemePreference } from "../lib/theme";
import { Icon, type IconName } from "./Icon";
import { ChoiceMenu } from "./Menu";

const OPTIONS: Record<ThemePreference, { text: string; icon: IconName }> = {
  system: { text: "System", icon: "system" },
  light: { text: "Light", icon: "sun" },
  dark: { text: "Dark", icon: "moon" },
};

const CHOICES = THEME_PREFERENCES.map((value) => ({ value, ...OPTIONS[value] }));

export function ThemeMenu() {
  const preference = useThemePreference();
  return (
    <ChoiceMenu label="Theme" value={preference} choices={CHOICES} onChange={setThemePreference} className="btn btn-ghost btn-icon btn-sm">
      <Icon name={OPTIONS[preference].icon} />
    </ChoiceMenu>
  );
}
