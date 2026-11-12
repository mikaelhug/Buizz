import type { ReactNode } from "react";
import type { Schemas } from "../api/client";
import { fmt, tone } from "../lib/format";
import { href } from "../lib/route";
import { Info } from "./Popover";
import { StatusPill } from "./Status";

export type Live = Schemas["LiveSummary"];
type Stats = Schemas["LiveStats"];
type Value = number | string | null | undefined;

export type Column = {
  label: string;
  about?: string;
  numeric?: boolean;
  ascending?: boolean;
  value: (live: Live) => Value;
  cell: (live: Live) => ReactNode;
};

export type LiveKey =
  | "bot"
  | "since"
  | "sessions"
  | "total_return"
  | "benchmark_return"
  | "excess_return"
  | "max_drawdown"
  | "volatility"
  | "beta"
  | "confidence"
  | "verdict"
  | "sessions_needed"
  | "status";

export type Sort<K extends string> = { key: K; descending: boolean };

const stat = <T,>(live: Live, pick: (stats: Stats) => T) => (live.run.live_stats ? pick(live.run.live_stats) : undefined);

function Signed({ value }: { value: number | null | undefined }) {
  return <span className={`num ${tone(value) ?? ""}`}>{fmt.change(value)}</span>;
}

function Excess({ live }: { live: Live }) {
  const stats = live.run.live_stats;
  return (
    <>
      <Signed value={stats?.excess_return} />
      {stats?.excess_error !== null && stats?.excess_error !== undefined && (
        <span className="se">
          <span aria-hidden="true">±</span>
          <span className="visually-hidden">plus or minus</span>
          {fmt.percent(stats.excess_error)}
        </span>
      )}
    </>
  );
}

export function retired(live: Live): boolean {
  return ["completed", "failed", "cancelled"].includes(live.run.status);
}

export function LiveStatus({ live }: { live: Live }) {
  return retired(live) ? <span className="pill pill-retired">retired</span> : <StatusPill status={live.run.status} />;
}

function Verdict({ live }: { live: Live }) {
  const verdict = stat(live, (stats) => stats.verdict) ?? "incubating";
  return <span className={`pill pill-${verdict}`}>{verdict}</span>;
}

const negative = (value: number | null | undefined) => (value === null || value === undefined ? value : -value);

export const LIVE_COLUMNS: Record<LiveKey, Column> = {
  bot: {
    label: "Bot",
    value: (live) => live.run.bot,
    cell: (live) => <a href={href({ page: "bot", name: live.run.bot, run: live.run.id })}>{live.run.bot}</a>,
  },
  since: {
    label: "Live since",
    value: (live) => live.run.created_at,
    cell: (live) => <span className="num">{fmt.date(live.run.created_at)}</span>,
  },
  sessions: {
    label: "Sessions",
    about: "Session closes since the run went live; paused sessions count, since the run keeps its positions marked while paused",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.sessions) ?? 0,
    cell: (live) => <span className="num">{fmt.integer(stat(live, (stats) => stats.sessions) ?? 0)}</span>,
  },
  total_return: {
    label: "Return",
    about: "Change in equity since the run went live, after every cost, through the latest session close",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.total_return),
    cell: (live) => <Signed value={stat(live, (stats) => stats.total_return)} />,
  },
  benchmark_return: {
    label: "SPY",
    about:
      "The bot's benchmark (SPY unless its bot.toml names another) bought at the close before the first live session and held to the same session close",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.benchmark_return),
    cell: (live) => <Signed value={stat(live, (stats) => stats.benchmark_return)} />,
  },
  excess_return: {
    label: "Excess",
    about:
      "Return minus the benchmark's over the same window. The ± figure is one standard error: the spread of daily excess returns times the square root of the sessions, so an excess within it of zero is not yet distinguishable from luck",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.excess_return),
    cell: (live) => <Excess live={live} />,
  },
  max_drawdown: {
    label: "Max DD",
    about: "Largest fall in session-close equity from its running peak since the run went live",
    numeric: true,
    ascending: true,
    value: (live) => stat(live, (stats) => stats.max_drawdown),
    cell: (live) => <Signed value={negative(stat(live, (stats) => stats.max_drawdown))} />,
  },
  volatility: {
    label: "Vol",
    about: "Annualised standard deviation of daily returns since the run went live",
    numeric: true,
    ascending: true,
    value: (live) => stat(live, (stats) => stats.volatility),
    cell: (live) => <span className="num">{fmt.percent(stat(live, (stats) => stats.volatility))}</span>,
  },
  beta: {
    label: "Beta",
    about: "How much of the daily moves followed the benchmark's: 1 moves with it, 0 ignores it",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.beta),
    cell: (live) => <span className="num">{fmt.ratio(stat(live, (stats) => stats.beta))}</span>,
  },
  confidence: {
    label: "Confidence",
    about:
      "Probabilistic Sharpe ratio of daily excess returns over the benchmark: the probability that the true Sharpe of the excess is above zero, allowing for the track length, skew and fat tails (Bailey and López de Prado)",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.probabilistic_sharpe),
    cell: (live) => <span className="num">{fmt.percent(stat(live, (stats) => stats.probabilistic_sharpe))}</span>,
  },
  verdict: {
    label: "Verdict",
    about: "Incubating until 20 live sessions, then tracking",
    value: (live) => stat(live, (stats) => stats.verdict) ?? "incubating",
    cell: (live) => <Verdict live={live} />,
  },
  sessions_needed: {
    label: "Still needed",
    about:
      "Sessions still needed before the excess return could count as proven at this confidence: the minimum track record length (Bailey and López de Prado) at 1 − 0.05 ÷ live runs started. Blank while the excess Sharpe is not positive",
    numeric: true,
    ascending: true,
    value: (live) => live.sessions_needed,
    cell: (live) => <span className="num">{fmt.integer(live.sessions_needed)}</span>,
  },
  status: {
    label: "Status",
    value: (live) => (retired(live) ? "retired" : live.run.status),
    cell: (live) => <LiveStatus live={live} />,
  },
};

export function compareValues(left: Value, right: Value, descending: boolean): number {
  if (left === null || left === undefined) {
    return right === null || right === undefined ? 0 : 1;
  }
  if (right === null || right === undefined) {
    return -1;
  }
  const order = typeof left === "number" && typeof right === "number" ? left - right : String(left).localeCompare(String(right));
  return descending ? -order : order;
}

export function nextSort<K extends string>(current: Sort<K>, key: K, ascending: boolean): Sort<K> {
  return current.key === key ? { key, descending: !current.descending } : { key, descending: !ascending };
}

type HeaderProps<K extends string> = { column: Pick<Column, "label" | "about" | "numeric">; columnKey: K; sort: Sort<K>; onSort: (key: K) => void; className?: string | undefined };

export function SortHeader<K extends string>({ column, columnKey, sort, onSort, className }: HeaderProps<K>) {
  const active = sort.key === columnKey;
  return (
    <th
      scope="col"
      className={`sortable${column.numeric ? " r" : ""}${active ? (sort.descending ? " desc" : " asc") : ""}${className ? ` ${className}` : ""}`}
      aria-sort={active ? (sort.descending ? "descending" : "ascending") : "none"}
    >
      <span className="th-inner">
        {column.about && <Info label={column.label}>{column.about}</Info>}
        <button type="button" onClick={() => onSort(columnKey)}>
          {column.label}
        </button>
      </span>
    </th>
  );
}
