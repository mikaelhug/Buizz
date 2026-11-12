import type { Schemas } from "../api/client";
import { fmt, tone } from "../lib/format";
import { Info } from "./Popover";

export type Metrics = Schemas["Metrics"];
export type MetricKey = keyof Metrics;

type Value = number | null | undefined;

type MetricSpec = {
  key: MetricKey;
  label: string;
  name: string;
  title: string;
  render: (value: Value) => string;
  tone?: (value: Value) => "up" | "down" | undefined;
  missing?: (metrics: Metrics) => string;
  caveat?: (metrics: Metrics) => string | undefined;
};

const TRADING_DAYS = 252;
const DAILY = "Computed from session-close equity, so intraday swings that recover before the close do not count.";

const drawdown = (value: Value) => (value === null || value === undefined ? value : -value);
const noEquity = () => "no equity yet";
const needsSessions = (metrics: Metrics) => (metrics.sessions < 2 ? "needs ≥ 2 sessions" : undefined);
const benchmarkReason = (metrics: Metrics) => needsSessions(metrics) ?? "no benchmark returns";
const needsYear = (metrics: Metrics) =>
  metrics.sessions < TRADING_DAYS ? `needs ≥ 1 year (${fmt.integer(metrics.sessions)} of ${TRADING_DAYS} sessions)` : undefined;
const noTrades = (metrics: Metrics) => (metrics.trades === 0 ? "no closed trades" : undefined);
const needsYearOr = (fallback: string) => (metrics: Metrics) => needsYear(metrics) ?? fallback;

export const METRICS: Record<MetricKey, MetricSpec> = {
  total_return: { key: "total_return", label: "Return", name: "Total return", title: "Change in equity from the start cash to the latest mark, after every cost.", render: fmt.change, tone, missing: noEquity },
  cagr: {
    key: "cagr",
    label: "CAGR",
    name: "CAGR",
    title: "Compound annual growth rate: the total return expressed per year. Shown once the run covers at least a year of sessions.",
    render: fmt.change,
    tone,
    missing: needsYearOr(noEquity()),
  },
  volatility: { key: "volatility", label: "Vol", name: "Volatility", title: `Annualised standard deviation of daily returns. ${DAILY}`, render: fmt.percent, missing: (metrics) => needsSessions(metrics) ?? "no returns yet" },
  sharpe: {
    key: "sharpe",
    label: "Sharpe",
    name: "Sharpe",
    title: `Annualised mean daily return in excess of the Treasury bill rate, divided by its standard deviation. ${DAILY}`,
    render: fmt.ratio,
    tone,
    missing: (metrics) => needsSessions(metrics) ?? "flat returns or no T-bill rate",
  },
  sortino: {
    key: "sortino",
    label: "Sortino",
    name: "Sortino",
    title: `Like Sharpe, but divided by downside deviation only, so upside swings are not penalised. ${DAILY}`,
    render: fmt.ratio,
    tone,
    missing: (metrics) => needsSessions(metrics) ?? "flat returns or no session below the T-bill rate",
  },
  max_drawdown: {
    key: "max_drawdown",
    label: "Max DD",
    name: "Max drawdown",
    title: `Largest fall in equity from a running peak, shown negative as on the drawdown chart. ${DAILY}`,
    render: (value) => fmt.change(drawdown(value)),
    tone: (value) => tone(drawdown(value)),
    missing: noEquity,
  },
  calmar: {
    key: "calmar",
    label: "Calmar",
    name: "Calmar",
    title: "CAGR divided by the size of the max drawdown. Needs a year of sessions and some drawdown.",
    render: fmt.ratio,
    tone,
    missing: needsYearOr("flat returns or no drawdown"),
  },
  win_rate: { key: "win_rate", label: "Win rate", name: "Win rate", title: "Share of closed trades that made money after costs.", render: fmt.percent, missing: () => "no closed trades" },
  profit_factor: {
    key: "profit_factor",
    label: "Profit factor",
    name: "Profit factor",
    title: "Gross profit of winning trades divided by gross loss of losing trades. Above 1 the bot makes more than it loses.",
    render: fmt.ratio,
    missing: (metrics) => noTrades(metrics) ?? "no losing trades",
  },
  exposure: {
    key: "exposure",
    label: "Exposure",
    name: "Exposure",
    title:
      "Average gross position value as a share of equity, sampled at each session close. An intraday bot that is flat by the close reads 0% however much it trades during the day.",
    render: fmt.percent,
    missing: noEquity,
    caveat: (metrics) => (metrics.exposure === 0 && metrics.trades > 0 ? "flat at every close" : undefined),
  },
  turnover: {
    key: "turnover",
    label: "Turnover",
    name: "Turnover",
    title: "Total traded value over the run divided by average equity: 2.0 means the book was bought and sold about once.",
    render: fmt.ratio,
    missing: noEquity,
  },
  alpha: {
    key: "alpha",
    label: "Alpha",
    name: "Alpha",
    title: `Annualised return not explained by the benchmark, from a regression of daily returns on the benchmark's. ${DAILY}`,
    render: fmt.change,
    tone,
    missing: benchmarkReason,
  },
  beta: {
    key: "beta",
    label: "Beta",
    name: "Beta",
    title: "Sensitivity of daily returns to the benchmark's: 1 moves with it, 0 is unrelated, negative moves against it.",
    render: fmt.ratio,
    missing: benchmarkReason,
  },
  trades: { key: "trades", label: "Trades", name: "Trades", title: "Closed round trips: a position opened and taken back to flat (or flipped).", render: fmt.integer },
  sessions: { key: "sessions", label: "Sessions", name: "Sessions", title: "Trading sessions with a close mark so far.", render: fmt.integer },
};

export function MetricValue({ metrics, metric }: { metrics: Metrics | null | undefined; metric: MetricKey }) {
  const spec = METRICS[metric];
  const value = metrics?.[metric];
  return <span className={`num ${spec.tone?.(value) ?? ""}`}>{spec.render(value)}</span>;
}

const BENCHMARKED: ReadonlySet<MetricKey> = new Set(["total_return", "cagr", "volatility", "sharpe", "sortino", "max_drawdown", "calmar"]);

type Benchmark = { symbol: string; metrics: Metrics };

export type Stat = { label: string; title: string; text: string; tone?: "up" | "down" | undefined; bench?: string | undefined; note?: string | undefined };

function metricStat(key: MetricKey, metrics: Metrics | null | undefined, benchmark: Benchmark | undefined): Stat {
  const spec = METRICS[key];
  const value = metrics?.[key];
  const reference = benchmark && BENCHMARKED.has(key) ? benchmark.metrics[key] : undefined;
  const absent = value === null || value === undefined;
  const note = metrics ? (absent ? spec.missing?.(metrics) : spec.caveat?.(metrics)) : "waiting for the first session";
  return {
    label: spec.name,
    title: spec.title,
    text: spec.render(value),
    tone: spec.tone?.(value),
    bench: benchmark && reference !== null && reference !== undefined ? `${benchmark.symbol} ${spec.render(reference)}` : undefined,
    note,
  };
}

function excessStat(metrics: Metrics | null | undefined, benchmark: Benchmark | undefined): Stat {
  const own = metrics?.total_return;
  const theirs = benchmark?.metrics.total_return;
  const excess = own === null || own === undefined || theirs === null || theirs === undefined ? undefined : own - theirs;
  return {
    label: "Excess return",
    title: `Total return minus the benchmark's buy and hold over the same period and start cash${benchmark ? ` (${benchmark.symbol})` : ""}.`,
    text: fmt.change(excess),
    tone: tone(excess),
    note: excess === undefined ? (benchmark ? "no benchmark return" : "once the run has finished") : undefined,
  };
}

function annualized(metrics: Metrics | null | undefined): number | undefined {
  const growth = metrics?.total_return;
  return growth === null || growth === undefined || !metrics || metrics.sessions < 1 ? undefined : Math.max(0, 1 + growth) ** (TRADING_DAYS / metrics.sessions) - 1;
}

function excessCagrStat(metrics: Metrics | null | undefined, benchmark: Benchmark | undefined): Stat {
  const sessions = metrics?.sessions ?? 0;
  const covered = benchmark?.metrics.sessions;
  const own = annualized(metrics);
  const theirs = covered === sessions ? annualized(benchmark?.metrics) : undefined;
  const excess = own === undefined || theirs === undefined ? undefined : own - theirs;
  const symbol = benchmark?.symbol ?? "benchmark";
  return {
    label: `CAGR vs ${symbol}`,
    title: `The run's annualized return minus ${symbol}'s buy and hold annualized over exactly the same sessions and start cash. Under ${TRADING_DAYS} sessions it is still annualized, but a few weeks annualized is not a track record.`,
    text: fmt.change(excess),
    tone: tone(excess),
    bench: excess === undefined ? undefined : `CAGR ${fmt.change(own)} · ${symbol} ${fmt.change(theirs)}`,
    note:
      excess === undefined
        ? !benchmark
          ? "once the run has finished"
          : covered !== sessions && own !== undefined
            ? `${benchmark.symbol} covers ${fmt.integer(covered)} of ${fmt.integer(sessions)} sessions`
            : "no benchmark return"
        : sessions < TRADING_DAYS
          ? `annualized from ${fmt.integer(sessions)} sessions`
          : undefined,
  };
}

export type MonthRange = { best: number; worst: number } | undefined;

function monthStat(text: "Best month" | "Worst month", value: number | undefined): Stat {
  return {
    label: text,
    title: `${text === "Best month" ? "Highest" : "Lowest"} calendar-month return; the first and last months may be partial.`,
    text: fmt.change(value),
    tone: tone(value),
    note: value === undefined ? "no month-end mark yet" : undefined,
  };
}

function StatRow({ stat, headline = false }: { stat: Stat; headline?: boolean }) {
  return (
    <div className={headline ? "metric metric-headline" : "metric"}>
      <dt>
        {stat.label}
        <Info label={stat.label}>{stat.title}</Info>
      </dt>
      <dd>
        <span className={headline ? `num display ${stat.tone ?? ""}` : "num"}>{stat.text}</span>
        {stat.bench && <span className="metric-bench">{stat.bench}</span>}
        {stat.note && <span className="metric-note">{stat.note}</span>}
      </dd>
    </div>
  );
}

type GroupsProps = { metrics: Metrics | null | undefined; benchmark: Benchmark | undefined; months: MonthRange };

export function MetricGroups({ metrics, benchmark, months }: GroupsProps) {
  const stat = (key: MetricKey) => metricStat(key, metrics, benchmark);
  const groups: { title: string; headline: Stat; rows: Stat[] }[] = [
    { title: "Returns", headline: stat("total_return"), rows: [stat("cagr"), monthStat("Best month", months?.best), monthStat("Worst month", months?.worst), stat("sessions")] },
    { title: "Risk", headline: stat("max_drawdown"), rows: [stat("volatility"), stat("sharpe"), stat("sortino"), stat("calmar")] },
    { title: "Trading", headline: stat("trades"), rows: [stat("win_rate"), stat("profit_factor"), stat("turnover"), stat("exposure")] },
    { title: `vs ${benchmark?.symbol ?? "benchmark"}`, headline: excessCagrStat(metrics, benchmark), rows: [excessStat(metrics, benchmark), stat("alpha"), stat("beta")] },
  ];
  return (
    <div className="metric-groups">
      {groups.map((group) => (
        <section key={group.title} className="metric-group" aria-label={group.title}>
          <h3>{group.title}</h3>
          <dl>
            <StatRow stat={group.headline} headline />
            {group.rows.map((row) => (
              <StatRow key={row.label} stat={row} />
            ))}
          </dl>
        </section>
      ))}
    </div>
  );
}

export function StatGrid({ stats }: { stats: Stat[] }) {
  return (
    <dl className="metric-grid">
      {stats.map((item) => (
        <StatRow key={item.label} stat={item} />
      ))}
    </dl>
  );
}
