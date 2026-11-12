import type { LineData, UTCTimestamp } from "lightweight-charts";
import type { Schemas } from "../../api/client";
import { exchangeWallSeconds, fmt } from "../../lib/format";

export type Point = Schemas["SparkPoint"] & Partial<Pick<Schemas["EquityPoint"], "gross_exposure">>;
export type Sample = LineData<UTCTimestamp>;
export type Line = Sample[];

export type Curves = { equity: Line; drawdown: Line; exposure: Line; peak: number };
export type Added = { equity: Sample; drawdown: Sample; exposure: Sample | undefined };

export const DAY_SECONDS = 86_400;

export function wallTime(at: string): UTCTimestamp {
  return exchangeWallSeconds(at) as UTCTimestamp;
}

function put(line: Line, sample: Sample) {
  if (line.at(-1)?.time === sample.time) {
    line[line.length - 1] = sample;
  } else {
    line.push(sample);
  }
}

export function add(curves: Curves, time: UTCTimestamp, value: number, grossExposure?: number): Added | undefined {
  const last = curves.equity.at(-1);
  if (last && time < last.time) {
    return undefined;
  }
  curves.peak = Math.max(curves.peak, value);
  const added: Added = {
    equity: { time, value },
    drawdown: { time, value: curves.peak > 0 ? value / curves.peak - 1 : 0 },
    exposure: grossExposure === undefined || value <= 0 ? undefined : { time, value: grossExposure / value },
  };
  put(curves.equity, added.equity);
  put(curves.drawdown, added.drawdown);
  if (added.exposure) {
    put(curves.exposure, added.exposure);
  }
  return added;
}

export function addPoint(curves: Curves, point: Point): Added | undefined {
  return add(curves, wallTime(point.at), Number(point.equity), point.gross_exposure === undefined ? undefined : Number(point.gross_exposure));
}

export function curves(points: Point[], origin?: Sample): Curves {
  const built: Curves = { equity: [], drawdown: [], exposure: [], peak: Number.NEGATIVE_INFINITY };
  if (origin) {
    add(built, origin.time, origin.value);
  }
  for (const point of points) {
    addPoint(built, point);
  }
  return built;
}

export function capitalOrigin(points: Point[], benchmark: Point[]): Sample | undefined {
  const first = points[0];
  const base = benchmark[0];
  if (!first || !base || wallTime(base.at) >= wallTime(first.at)) {
    return undefined;
  }
  return { time: wallTime(base.at), value: Number(base.equity) };
}

function countBefore(line: { time: number }[], time: number, inclusive: boolean): number {
  let low = 0;
  let high = line.length;
  while (low < high) {
    const middle = (low + high) >> 1;
    const sample = line[middle]?.time ?? 0;
    if (sample < time || (inclusive && sample === time)) {
      low = middle + 1;
    } else {
      high = middle;
    }
  }
  return low;
}

export function asOf<T extends { time: number }>(line: T[], time: number): T | undefined {
  return line[countBefore(line, time, true) - 1];
}

export function firstFrom<T extends { time: number }>(line: T[], time: number): T | undefined {
  return line[countBefore(line, time, false)];
}

export function isIntraday(line: Line): boolean {
  return line.some((sample, index) => index > 0 && sample.time - (line[index - 1]?.time ?? 0) < DAY_SECONDS);
}

export const PRESETS = ["1M", "3M", "6M", "YTD", "1Y", "5Y", "All"] as const;
export type Preset = (typeof PRESETS)[number];

const PRESET_SLACK_SECONDS = 7 * DAY_SECONDS;
const PRESET_MONTHS: Partial<Record<Preset, number>> = { "1M": 1, "3M": 3, "6M": 6, "1Y": 12, "5Y": 60 };

export function presetStart(preset: Preset, last: number): number | undefined {
  const date = new Date(last * 1000);
  if (preset === "YTD") {
    return Date.UTC(date.getUTCFullYear(), 0, 1) / 1000;
  }
  const months = PRESET_MONTHS[preset];
  if (months === undefined) {
    return undefined;
  }
  date.setUTCMonth(date.getUTCMonth() - months);
  return date.getTime() / 1000;
}

export function presetsWithin(first: number, last: number): Preset[] {
  return PRESETS.filter((preset) => {
    const start = presetStart(preset, last);
    return start === undefined || start - first > PRESET_SLACK_SECONDS;
  });
}

export type MarkKind = "stop" | "breach" | "pause" | "resume";
export type Mark = { id: string; time: UTCTimestamp; kind: MarkKind; text: string };

const LIMIT_NAMES: Record<Schemas["RiskLimit"], string> = {
  max_drawdown: "drawdown",
  max_position_weight: "position weight",
  max_gross_exposure: "gross exposure",
};

function describe(kind: Schemas["RunEventKind"], paused: boolean): Pick<Mark, "kind" | "text"> | undefined {
  if (kind.type === "risk_breach") {
    const reading = `${LIMIT_NAMES[kind.limit]} ${fmt.percent(kind.value)} reached the ${fmt.percent(kind.threshold)} limit`;
    return kind.limit === "max_drawdown" ? { kind: "stop", text: `Drawdown stop: ${reading}` } : { kind: "breach", text: `Risk breach: ${reading}` };
  }
  if (kind.type !== "status") {
    return undefined;
  }
  const detail = kind.message ? `: ${kind.message}` : "";
  if (kind.status === "paused") {
    return { kind: "pause", text: `Paused${detail}` };
  }
  if (kind.status === "running" && paused) {
    return { kind: "resume", text: `Resumed${detail}` };
  }
  return undefined;
}

export function eventMarks(events: Schemas["RunEvent"][], line: Line): Mark[] {
  const first = line[0];
  if (!first) {
    return [];
  }
  const marks: Mark[] = [];
  let paused = false;
  for (const event of events) {
    const described = describe(event.kind, paused);
    if (event.kind.type === "status") {
      paused = event.kind.status === "paused";
    }
    if (described) {
      const time = (asOf(line, wallTime(event.clock ?? event.at)) ?? first).time;
      marks.push({ id: String(event.seq), time, ...described });
    }
  }
  return marks.sort((left, right) => left.time - right.time);
}

