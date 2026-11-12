import type { ReactNode } from "react";
import type { Schemas } from "../api/client";
import { fmt, tone } from "../lib/format";
import { href } from "../lib/route";
import { Info } from "./Popover";
import { StatusPill } from "./Status";
import { SortHeader, type SortColumns, type Sorting, type SortValue } from "./Table";

export type Live = Schemas["LiveSummary"];
type Stats = Schemas["LiveStats"];

export type Column = {
  label: string;
  about?: string;
  numeric?: boolean;
  ascending?: boolean;
  value: (live: Live) => SortValue;
  cell: (live: Live) => ReactNode;
};

export type LiveKey =
  | "bot"
  | "since"
  | "sessions"
  | "excess_cagr"
  | "cagr"
  | "benchmark_cagr"
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

const stat = <T,>(live: Live, pick: (stats: Stats) => T) => (live.run.live_stats ? pick(live.run.live_stats) : undefined);

function Signed({ value }: { value: number | null | undefined }) {
  return <span className={`num ${tone(value) ?? ""}`}>{fmt.change(value)}</span>;
}

export const YEAR_SESSIONS = 252;

function PlusMinus({ value }: { value: number | null | undefined }) {
  return value === null || value === undefined ? null : (
    <span className="se">
      <span aria-hidden="true">±</span>
      <span className="visually-hidden">plus or minus</span>
      {fmt.percent(value)}
    </span>
  );
}

function Excess({ live }: { live: Live }) {
  const stats = live.run.live_stats;
  return (
    <>
      <Signed value={stats?.excess_return} />
      <PlusMinus value={stats?.excess_error} />
    </>
  );
}

export function Annualized({ value, sessions, error }: { value: number | null | undefined; sessions: number; error?: number | null | undefined }) {
  const short = value !== null && value !== undefined && sessions < YEAR_SESSIONS;
  return (
    <span className={short ? "annualized-short" : undefined} title={short ? `annualized from ${fmt.integer(sessions)} sessions` : undefined}>
      <Signed value={value} />
      <PlusMinus value={error} />
      {short && <span className="annualized-note">from {fmt.integer(sessions)} sessions</span>}
    </span>
  );
}

const ANNUALIZED = `Annualized over the sessions since the run went live. Under ${YEAR_SESSIONS} sessions it is greyed and says how many sessions it comes from: a few weeks annualized is not a track record`;

function annualizedColumn(label: string, about: string, pick: (stats: Stats) => number, error?: (stats: Stats) => number | null | undefined): Column {
  return {
    label,
    about: `${about}. ${ANNUALIZED}`,
    numeric: true,
    value: (live) => stat(live, pick),
    cell: (live) => (
      <Annualized value={stat(live, pick)} sessions={stat(live, (stats) => stats.sessions) ?? 0} error={error && stat(live, error)} />
    ),
  };
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
    value: (live) => Date.parse(live.run.created_at),
    cell: (live) => <span className="num">{fmt.date(live.run.created_at)}</span>,
  },
  sessions: {
    label: "Sessions",
    about: "Session closes since the run went live; paused sessions count, since the run keeps its positions marked while paused",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.sessions),
    cell: (live) => <span className="num">{fmt.integer(stat(live, (stats) => stats.sessions))}</span>,
  },
  excess_cagr: annualizedColumn(
    "CAGR vs SPY",
    "The run's CAGR minus its benchmark's CAGR over exactly the same sessions. The ± figure is one standard error of the annualized mean daily excess return",
    (stats) => stats.excess_cagr,
    (stats) => stats.excess_cagr_error,
  ),
  cagr: annualizedColumn("CAGR", "Compound annual growth of equity since the run went live", (stats) => stats.cagr),
  benchmark_cagr: annualizedColumn("SPY CAGR", "Compound annual growth of the benchmark over the same sessions", (stats) => stats.benchmark_cagr),
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

export function liveSortColumns<K extends LiveKey>(keys: readonly K[]): SortColumns<Live, K> {
  return Object.fromEntries(keys.map((key) => [key, { value: LIVE_COLUMNS[key].value, descending: !LIVE_COLUMNS[key].ascending && LIVE_COLUMNS[key].numeric === true }])) as SortColumns<
    Live,
    K
  >;
}

export function LiveHeader<S extends string>({ sorting, column, className }: { sorting: Sorting<Live, S>; column: LiveKey & S; className?: string | undefined }) {
  const spec = LIVE_COLUMNS[column];
  return (
    <SortHeader sorting={sorting} column={column} numeric={spec.numeric ?? false} className={className} info={spec.about && <Info label={spec.label}>{spec.about}</Info>}>
      {spec.label}
    </SortHeader>
  );
}
