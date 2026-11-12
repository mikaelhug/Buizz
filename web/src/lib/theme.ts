import { useSyncExternalStore } from "react";

export type ThemePreference = "system" | "light" | "dark";
export type ColorScheme = "light" | "dark";

export const THEME_PREFERENCES: readonly ThemePreference[] = ["system", "light", "dark"];

const STORAGE_KEY = "buizz-theme";
const dark = window.matchMedia("(prefers-color-scheme: dark)");
const listeners = new Set<() => void>();

function stored(): ThemePreference {
  try {
    const value = localStorage.getItem(STORAGE_KEY);
    return THEME_PREFERENCES.find((preference) => preference === value) ?? "system";
  } catch {
    return "system";
  }
}

let preference = stored();

function apply() {
  const root = document.documentElement;
  if (preference === "system") {
    delete root.dataset.theme;
  } else {
    root.dataset.theme = preference;
  }
  listeners.forEach((listener) => listener());
}

apply();
dark.addEventListener("change", apply);
window.addEventListener("storage", (event) => {
  if (event.key === STORAGE_KEY) {
    preference = stored();
    apply();
  }
});

export function setThemePreference(next: ThemePreference) {
  preference = next;
  try {
    if (next === "system") {
      localStorage.removeItem(STORAGE_KEY);
    } else {
      localStorage.setItem(STORAGE_KEY, next);
    }
  } catch {}
  apply();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

export function useThemePreference(): ThemePreference {
  return useSyncExternalStore(subscribe, () => preference);
}

export function useColorScheme(): ColorScheme {
  return useSyncExternalStore(subscribe, () => (preference === "system" ? (dark.matches ? "dark" : "light") : preference));
}

export function themeColor(token: string, alpha = 1): string {
  const probe = document.createElement("span");
  probe.style.display = "none";
  probe.style.color = `var(${token})`;
  document.body.append(probe);
  const [red, green, blue] = getComputedStyle(probe).color.match(/[\d.]+/g) ?? [];
  probe.remove();
  return `rgba(${red ?? 0}, ${green ?? 0}, ${blue ?? 0}, ${alpha})`;
}

export function themeFont(): string {
  return getComputedStyle(document.documentElement).getPropertyValue("--font").trim();
}
