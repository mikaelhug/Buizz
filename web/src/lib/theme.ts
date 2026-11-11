import { useSyncExternalStore } from "react";

const dark = window.matchMedia("(prefers-color-scheme: dark)");

function subscribe(listener: () => void) {
  dark.addEventListener("change", listener);
  return () => dark.removeEventListener("change", listener);
}

export function useColorScheme(): "light" | "dark" {
  return useSyncExternalStore(subscribe, () => (dark.matches ? "dark" : "light"));
}
