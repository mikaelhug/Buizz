import { type MouseEvent, type ReactNode, useEffect, useEffectEvent, useRef, useState } from "react";
import { COMPARE_MAX, href } from "../lib/route";

type Key = string | number;

export type Selection<K extends Key> = {
  keys: K[];
  has: (key: K) => boolean;
  toggle: (key: K, range: boolean) => void;
  toggleAll: () => void;
  clear: () => void;
  all: boolean;
};

export function useSelection<K extends Key>(visible: readonly K[]): Selection<K> {
  const [chosen, setChosen] = useState<ReadonlySet<K>>(new Set());
  const anchor = useRef<K | undefined>(undefined);
  const keys = visible.filter((key) => chosen.has(key));
  const all = visible.length > 0 && keys.length === visible.length;

  const toggle = (key: K, range: boolean) => {
    const from = anchor.current === undefined ? -1 : visible.indexOf(anchor.current);
    const to = visible.indexOf(key);
    const on = !chosen.has(key);
    const span = range && from >= 0 && to >= 0 ? visible.slice(Math.min(from, to), Math.max(from, to) + 1) : [key];
    anchor.current = key;
    setChosen((current) => {
      const next = new Set(current);
      span.forEach((item) => (on ? next.add(item) : next.delete(item)));
      return next;
    });
  };

  return {
    keys,
    has: (key) => chosen.has(key),
    toggle,
    toggleAll: () => setChosen(all ? new Set() : new Set(visible)),
    clear: () => setChosen(new Set()),
    all,
  };
}

export function SelectBox<K extends Key>({ selection, value, label }: { selection: Selection<K>; value: K; label: string }) {
  return (
    <input
      type="checkbox"
      className="select-box"
      aria-label={label}
      checked={selection.has(value)}
      onChange={() => {}}
      onClick={(event: MouseEvent<HTMLInputElement>) => selection.toggle(value, event.shiftKey)}
    />
  );
}

export function SelectAll<K extends Key>({ selection, label }: { selection: Selection<K>; label: string }) {
  const ref = useRef<HTMLInputElement>(null);
  const partial = selection.keys.length > 0 && !selection.all;
  useEffect(() => {
    if (ref.current) {
      ref.current.indeterminate = partial;
    }
  }, [partial]);
  return <input ref={ref} type="checkbox" className="select-box" aria-label={label} checked={selection.all} onChange={selection.toggleAll} />;
}

export function BulkBar<K extends Key>({ selection, noun, children }: { selection: Selection<K>; noun: string; children: ReactNode }) {
  const count = selection.keys.length;
  const clear = useEffectEvent(() => selection.clear());
  useEffect(() => {
    if (count === 0) {
      return;
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape" && !document.querySelector("dialog[open], .popover")) {
        clear();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [count]);

  if (count === 0) {
    return null;
  }
  return (
    <div className="bulk-bar" role="toolbar" aria-label={`Selected ${noun}`}>
      <span className="bulk-count">
        <strong className="num">{count}</strong> {noun} selected
      </span>
      <span className="bulk-actions">{children}</span>
      <button type="button" className="btn btn-sm btn-ghost" onClick={selection.clear}>
        Clear <kbd>Esc</kbd>
      </button>
    </div>
  );
}

export function CompareButton({ runs }: { runs: number[] }) {
  const ready = runs.length >= 2 && runs.length <= COMPARE_MAX;
  const hint = runs.length < 2 ? "Select at least 2 runs to compare" : `Compare at most ${COMPARE_MAX} runs`;
  return ready ? (
    <a className="btn btn-sm btn-primary" href={href({ page: "compare", runs })}>
      Compare {runs.length}
    </a>
  ) : (
    <button type="button" className="btn btn-sm" disabled title={hint}>
      Compare
    </button>
  );
}
