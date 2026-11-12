import { type AnimationEvent, type KeyboardEvent, type MouseEvent, type ReactNode, useEffect, useEffectEvent, useRef, useState } from "react";
import { fmt } from "../lib/format";
import { useStored } from "../lib/stored";

export type SortValue = string | number | null | undefined;
export type SortColumns<T, K extends string> = Record<K, { value: (row: T) => SortValue; descending?: boolean }>;
export type Sort<K extends string> = { key: K; descending: boolean };
export type Sorting<T, K extends string> = { sort: Sort<K>; toggle: (key: K) => void; order: (rows: readonly T[]) => T[] };

const collator = new Intl.Collator("en", { numeric: true, sensitivity: "base" });

function missing(value: SortValue): value is null | undefined {
  return value === null || value === undefined || value === "" || Number.isNaN(value);
}

function compare(a: string | number, b: string | number): number {
  return typeof a === "number" && typeof b === "number" ? a - b : collator.compare(String(a), String(b));
}

export function useSort<T, K extends string>(table: string, columns: SortColumns<T, K>, initial: Sort<NoInfer<K>>): Sorting<T, K> {
  const accept = (value: unknown): value is Sort<K> =>
    typeof value === "object" && value !== null && "key" in value && "descending" in value && typeof value.descending === "boolean" && Object.hasOwn(columns, String(value.key));
  const [sort, setSort] = useStored(`sort-${table}`, initial, accept);
  const toggle = (key: K) => setSort(sort.key === key ? { key, descending: !sort.descending } : { key, descending: columns[key].descending ?? false });
  const order = (rows: readonly T[]) => {
    const value = columns[sort.key].value;
    const direction = sort.descending ? -1 : 1;
    return rows
      .map((row) => ({ row, key: value(row) }))
      .toSorted((a, b) => (missing(a.key) ? (missing(b.key) ? 0 : 1) : missing(b.key) ? -1 : direction * compare(a.key, b.key)))
      .map(({ row }) => row);
  };
  return { sort, toggle, order };
}

type SortHeaderProps<K extends string> = {
  sorting: { sort: Sort<K>; toggle: (key: K) => void };
  column: K;
  numeric?: boolean;
  className?: string | undefined;
  info?: ReactNode;
  select?: ReactNode;
  rowSpan?: number | undefined;
  children: ReactNode;
};

export function SortHeader<K extends string>({ sorting, column, numeric = false, className, info, select, rowSpan, children }: SortHeaderProps<K>) {
  const active = sorting.sort.key === column;
  const direction = sorting.sort.descending ? "descending" : "ascending";
  const classes = ["sortable", numeric ? "r" : "", active ? (sorting.sort.descending ? "desc" : "asc") : "", className ?? ""].filter(Boolean).join(" ");
  return (
    <th scope="col" className={classes} rowSpan={rowSpan} aria-sort={active ? direction : undefined}>
      <span className="th-inner">
        {select}
        {numeric && info}
        <button type="button" onClick={() => sorting.toggle(column)}>
          {children}
        </button>
        {!numeric && info}
      </span>
    </th>
  );
}

export function TableFilter({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  return <input className="input input-sm table-filter" type="search" placeholder="Filter" aria-label={label} value={value} onChange={(event) => onChange(event.target.value)} />;
}

export function PageSearch({ label, value, onChange }: { label: string; value: string; onChange: (value: string) => void }) {
  return (
    <span className="page-search">
      <input
        className="input input-sm"
        type="search"
        placeholder={label}
        aria-label={label}
        aria-keyshortcuts="/"
        data-page-search=""
        value={value}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={(event) => event.key === "Escape" && event.currentTarget.blur()}
      />
      <kbd aria-hidden="true">/</kbd>
    </span>
  );
}

export function matches(query: string, ...fields: (string | number | null | undefined)[]): boolean {
  const needle = query.trim().toLowerCase();
  return !needle || fields.some((field) => field !== null && field !== undefined && String(field).toLowerCase().includes(needle));
}

const INTERACTIVE = "a, button, input, select, textarea, label, dialog, [role='dialog']";

function openUnlessInteractive(event: MouseEvent, open: () => void) {
  if (!(event.target as Element).closest(INTERACTIVE) && !window.getSelection()?.toString()) {
    open();
  }
}

export function openRow(target: string) {
  return (event: MouseEvent) =>
    openUnlessInteractive(event, () => {
      window.location.hash = target;
    });
}

const ROW_MOVES: Record<string, (rows: number, at: number) => number> = {
  ArrowDown: (rows, at) => Math.min(rows - 1, at + 1),
  ArrowUp: (_, at) => Math.max(0, at - 1),
  Home: () => 0,
  End: (rows) => rows - 1,
};

const FOCUSABLE = "a[href], button";

export function moveBetweenRows(event: KeyboardEvent<HTMLTableSectionElement>) {
  const move = ROW_MOVES[event.key];
  const cell = (event.target as Element).closest("td, th");
  const row = cell?.parentElement;
  if (!move || !cell || !(row instanceof HTMLTableRowElement) || event.altKey || event.ctrlKey || event.metaKey) {
    return;
  }
  const rows = [...event.currentTarget.rows];
  const next = rows[move(rows.length, rows.indexOf(row))];
  const column = [...row.cells].indexOf(cell as HTMLTableCellElement);
  const target = next?.cells[column]?.querySelector<HTMLElement>(FOCUSABLE) ?? next?.querySelector<HTMLElement>(FOCUSABLE);
  if (target) {
    event.preventDefault();
    target.focus();
    target.scrollIntoView({ block: "nearest" });
  }
}

type Key = string | number;

export type Flash = {
  row: (key: Key) => { "data-flash"?: "a" | "b"; onAnimationEnd?: (event: AnimationEvent) => void };
  quietly: (loadMore: (() => void) | undefined) => (() => void) | undefined;
};

export function useFlash<T>(rows: readonly T[] | undefined, keyOf: (row: T) => Key): Flash {
  const seen = useRef<Map<Key, T> | undefined>(undefined);
  const loadingMore = useRef(false);
  const [lit, setLit] = useState<ReadonlyMap<Key, number>>(new Map());
  const keyFor = useEffectEvent(keyOf);

  useEffect(() => {
    if (!rows) {
      seen.current = undefined;
      return;
    }
    const before = seen.current;
    const now = new Map(rows.map((row) => [keyFor(row), row]));
    seen.current = now;
    if (!before) {
      return;
    }
    const added = [...now.keys()].filter((key) => !before.has(key));
    const quiet = loadingMore.current && added.length > 0;
    if (quiet) {
      loadingMore.current = false;
    }
    const changed = [...now].flatMap(([key, row]) => {
      const old = before.get(key);
      return (old === undefined ? !quiet : old !== row && JSON.stringify(old) !== JSON.stringify(row)) ? [key] : [];
    });
    if (changed.length > 0) {
      setLit((current) => new Map([...current, ...changed.map((key): [Key, number] => [key, (current.get(key) ?? 0) + 1])]));
    }
  }, [rows]);

  const settle = (key: Key) =>
    setLit((current) => {
      const next = new Map(current);
      next.delete(key);
      return next;
    });

  return {
    row: (key) => {
      const times = lit.get(key);
      return times === undefined
        ? {}
        : { "data-flash": times % 2 ? "a" : "b", onAnimationEnd: (event) => event.target === event.currentTarget && settle(key) };
    },
    quietly: (loadMore) =>
      loadMore &&
      (() => {
        loadingMore.current = true;
        loadMore();
      }),
  };
}

const NOW_TICK_MS = 15_000;
const nowListeners = new Set<() => void>();
let ticker: ReturnType<typeof setInterval> | undefined;

function subscribeNow(listener: () => void) {
  nowListeners.add(listener);
  ticker ??= setInterval(() => nowListeners.forEach((notify) => notify()), NOW_TICK_MS);
  return () => {
    nowListeners.delete(listener);
    if (nowListeners.size === 0) {
      clearInterval(ticker);
      ticker = undefined;
    }
  };
}

const relative = new Intl.RelativeTimeFormat("en", { numeric: "auto", style: "short" });
const UNITS: readonly [Intl.RelativeTimeFormatUnit, number][] = [
  ["day", 86_400_000],
  ["hour", 3_600_000],
  ["minute", 60_000],
];
const JUST_NOW_MS = 45_000;
const RELATIVE_DAYS = 7;

function ago(at: string, from: number): string {
  const delta = Date.parse(at) - from;
  if (Math.abs(delta) < JUST_NOW_MS) {
    return "just now";
  }
  if (Math.abs(delta) > RELATIVE_DAYS * 86_400_000) {
    return fmt.local(at);
  }
  const [unit, size] = UNITS.find(([, ms]) => Math.abs(delta) >= ms) ?? ["minute", 60_000];
  return relative.format(Math.round(delta / size), unit);
}

export function Ago({ at }: { at: string }) {
  const [current, setCurrent] = useState(Date.now);
  useEffect(() => subscribeNow(() => setCurrent(Date.now())), []);
  return (
    <time dateTime={at} title={`${fmt.local(at)} local`}>
      {ago(at, current)}
    </time>
  );
}
