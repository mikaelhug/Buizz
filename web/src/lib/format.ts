export const ABSENT = "—";

const money = new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const price = new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 4 });
const ratio = new Intl.NumberFormat("en-US", { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const percent = new Intl.NumberFormat("en-US", {
  style: "percent",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});
const signedPercent = new Intl.NumberFormat("en-US", {
  style: "percent",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
  signDisplay: "exceptZero",
});
const integer = new Intl.NumberFormat("en-US");
const dollars = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", notation: "compact", maximumFractionDigits: 1 });
const EXCHANGE_ZONE = "America/New_York";
const minutes = { year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit" } as const;
const localTime = new Intl.DateTimeFormat("sv-SE", minutes);
const localDate = new Intl.DateTimeFormat("sv-SE", { year: "numeric", month: "2-digit", day: "2-digit" });
const exchangeTime = new Intl.DateTimeFormat("sv-SE", { ...minutes, timeZone: EXCHANGE_ZONE });
const exchangeParts = new Intl.DateTimeFormat("en-US", {
  timeZone: EXCHANGE_ZONE,
  hourCycle: "h23",
  year: "numeric",
  month: "numeric",
  day: "numeric",
  hour: "numeric",
  minute: "numeric",
  second: "numeric",
});

const HOUR_MS = 3_600_000;
const exchangeOffsets = new Map<number, number>();

function exchangeOffsetSeconds(hour: number): number {
  const cached = exchangeOffsets.get(hour);
  if (cached !== undefined) {
    return cached;
  }
  const parts = exchangeParts.formatToParts(new Date(hour * HOUR_MS));
  const part = (type: Intl.DateTimeFormatPartTypes) => Number(parts.find((candidate) => candidate.type === type)?.value);
  const wall = Date.UTC(part("year"), part("month") - 1, part("day"), part("hour"), part("minute"), part("second"));
  const offset = (wall - hour * HOUR_MS) / 1000;
  exchangeOffsets.set(hour, offset);
  return offset;
}

export function exchangeWallSeconds(at: string): number {
  const ms = Date.parse(at);
  return Math.floor(ms / 1000) + exchangeOffsetSeconds(Math.floor(ms / HOUR_MS));
}

type Maybe<T> = T | null | undefined;

function present<T>(value: Maybe<T>, render: (value: T) => string): string {
  return value === null || value === undefined ? ABSENT : render(value);
}

export const fmt = {
  money: (value: Maybe<string | number>) => present(value, (v) => money.format(Number(v))),
  price: (value: Maybe<string | number>) => present(value, (v) => price.format(Number(v))),
  ratio: (value: Maybe<number>) => present(value, (v) => ratio.format(v)),
  percent: (value: Maybe<string | number>) => present(value, (v) => percent.format(Number(v))),
  change: (value: Maybe<number>) => present(value, (v) => signedPercent.format(v)),
  integer: (value: Maybe<number>) => present(value, (v) => integer.format(v)),
  dollars: (value: Maybe<number>) => present(value, (v) => dollars.format(v)),
  local: (value: Maybe<string>) => present(value, (v) => localTime.format(new Date(v))),
  date: (value: Maybe<string>) => present(value, (v) => localDate.format(new Date(v))),
  market: (value: Maybe<string>) => present(value, (v) => `${exchangeTime.format(new Date(v))} ET`),
  memory: (bytes: Maybe<number>) => present(bytes, (v) => `${integer.format(Math.round(v / MIB))} MiB`),
  bytes: (bytes: Maybe<number>) => present(bytes, sized),
  duration: (seconds: Maybe<number>) => present(seconds, duration),
};

const MIB = 1024 * 1024;
const SIZE_UNITS = ["B", "KB", "MB", "GB", "TB"];
const oneDecimal = new Intl.NumberFormat("en-US", { maximumFractionDigits: 1 });

function sized(bytes: number): string {
  const step = Math.min(SIZE_UNITS.length - 1, Math.max(0, Math.floor(Math.log(Math.max(bytes, 1)) / Math.log(1000))));
  return `${oneDecimal.format(bytes / 1000 ** step)} ${SIZE_UNITS[step]}`;
}

function duration(seconds: number): string {
  if (seconds < 60) {
    return `${ratio.format(seconds)} s`;
  }
  const minutes = Math.round(seconds / 60);
  return minutes < 60 ? `${minutes} min` : `${Math.floor(minutes / 60)} h ${minutes % 60} min`;
}

export function tone(value: Maybe<number>): "up" | "down" | undefined {
  if (value === null || value === undefined || value === 0) {
    return undefined;
  }
  return value > 0 ? "up" : "down";
}

export function paramsText(params: Record<string, unknown>): string {
  return Object.entries(params)
    .map(([key, value]) => `${key}=${JSON.stringify(value)}`)
    .join(" ");
}

export function label(value: string): string {
  return value.replaceAll("_", " ");
}
