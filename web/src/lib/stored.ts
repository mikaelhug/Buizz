import { useState } from "react";

const PREFIX = "buizz-";

function read<T>(key: string, accept: (value: unknown) => value is T): T | undefined {
  try {
    const raw = localStorage.getItem(PREFIX + key);
    const value: unknown = raw === null ? undefined : JSON.parse(raw);
    return accept(value) ? value : undefined;
  } catch {
    return undefined;
  }
}

function write(key: string, value: unknown) {
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {}
}

export function useStored<T>(key: string, initial: T, accept: (value: unknown) => value is T): [T, (next: T) => void] {
  const [value, setValue] = useState(() => read(key, accept) ?? initial);
  const update = (next: T) => {
    setValue(next);
    write(key, next);
  };
  return [value, update];
}
