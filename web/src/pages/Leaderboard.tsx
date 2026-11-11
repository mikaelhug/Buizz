import { useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt, paramsText } from "../lib/format";
import { href, PRESETS, type Preset } from "../lib/route";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { FEW_TRADES, type MetricKey, METRICS, type Metrics, MetricValue, sharpeError } from "../ui/Metrics";
import { RunBadges } from "../ui/Status";

const COLUMNS: MetricKey[] = [
  "total_return",
  "cagr",
  "volatility",
  "sharpe",
  "sortino",
  "max_drawdown",
  "calmar",
  "win_rate",
  "profit_factor",
  "exposure",
  "alpha",
  "beta",
  "trades",
];

const TRADE_METRICS: ReadonlySet<MetricKey> = new Set(["win_rate", "profit_factor", "trades"]);

const ASCENDING: ReadonlySet<MetricKey> = new Set(["volatility", "max_drawdown"]);

type Sort = { key: MetricKey; descending: boolean };
type Row = Schemas["LeaderboardRow"];
type Entry = { kind: "bot"; row: Row } | { kind: "benchmark"; benchmark: Schemas["LeaderboardBenchmark"] };

function metricsOf(entry: Entry): Metrics {
  return entry.kind === "bot" ? entry.row.metrics : entry.benchmark.metrics;
}

function sorted(entries: Entry[], sort: Sort): Entry[] {
  return entries.toSorted((a, b) => {
    const left = metricsOf(a)[sort.key];
    const right = metricsOf(b)[sort.key];
    if (left === null || left === undefined) {
      return right === null || right === undefined ? 0 : 1;
    }
    if (right === null || right === undefined) {
      return -1;
    }
    return sort.descending ? right - left : left - right;
  });
}

function costSummary(cost: Schemas["CostModel"]): string {
  return `${cost.slippage_bps} bps slippage · ${cost.commission_per_share}/sh min ${cost.commission_min} · ADV ${fmt.percent(cost.adv_participation)} · borrow ${fmt.percent(cost.borrow_rate)}`;
}

function cellTitle(metrics: Metrics, key: MetricKey): string | undefined {
  if (key === "sharpe") {
    const error = sharpeError(metrics);
    return error === undefined ? undefined : `±${fmt.ratio(error)} standard error over ${fmt.integer(metrics.sessions)} sessions`;
  }
  if (TRADE_METRICS.has(key) && metrics.trades < FEW_TRADES) {
    return `Only ${fmt.integer(metrics.trades)} closed trades`;
  }
  return undefined;
}

function MetricCells({ metrics, trading }: { metrics: Metrics; trading: boolean }) {
  return COLUMNS.map((key) => {
    const thin = trading && TRADE_METRICS.has(key) && metrics.trades < FEW_TRADES;
    const title = trading ? cellTitle(metrics, key) : undefined;
    return (
      <td key={key} className={`r${thin ? " thin" : ""}${title ? " help" : ""}`} title={title}>
        {trading || !TRADE_METRICS.has(key) ? <MetricValue metrics={metrics} metric={key} /> : null}
      </td>
    );
  });
}

export function Leaderboard({ preset }: { preset: Preset }) {
  const board = useResource(() => must(api.GET("/api/leaderboard", { params: { query: { preset } } })), [preset]);
  const [sort, setSort] = useState<Sort>({ key: "sharpe", descending: true });
  const [filter, setFilter] = useState("");

  useSpineEvents((event) => {
    if (event.type === "run_status" && (event.status === "completed" || event.status === "failed")) {
      board.reload();
    }
  });

  const toggle = (key: MetricKey) =>
    setSort((current) => (current.key === key ? { key, descending: !current.descending } : { key, descending: !ASCENDING.has(key) }));

  const query = filter.trim().toLowerCase();
  const rows = board.data?.rows ?? [];
  const shared = rows[0]?.badges.filter((badge) => rows.every((row) => row.badges.includes(badge))) ?? [];
  const entries = board.data
    ? sorted([...board.data.rows.map((row): Entry => ({ kind: "bot", row })), ...board.data.benchmarks.map((benchmark): Entry => ({ kind: "benchmark", benchmark }))], sort)
    : [];
  let rank = 0;
  const ranked = entries.map((entry) => ({ entry, rank: entry.kind === "bot" ? ++rank : undefined }));
  const shown = ranked.filter(({ entry }) => entry.kind === "benchmark" || entry.row.bot.toLowerCase().includes(query));

  return (
    <div className="leaderboard">
      <header className="page-head">
        <h1>Leaderboard</h1>
        <nav className="segmented" aria-label="Period">
          {PRESETS.map((option) => (
            <a key={option} href={href({ page: "leaderboard", preset: option })} className={option === preset ? "active" : undefined}>
              {option}
            </a>
          ))}
        </nav>
        {board.data && (
          <span className="muted">
            {board.data.period.from} → {board.data.period.to} · {costSummary(board.data.cost)}
          </span>
        )}
        <RunBadges badges={shared} />
        {rows.length > 0 && (
          <input
            className="input input-sm page-filter"
            type="search"
            placeholder="Filter bots"
            aria-label="Filter bots"
            value={filter}
            onChange={(event) => setFilter(event.target.value)}
          />
        )}
      </header>
      <StaleFailure resource={board} />
      {!board.data ? (
        board.error ? <Failure message={board.error} onRetry={board.reload} /> : <Loading />
      ) : board.data.rows.length === 0 ? (
        <p className="empty">No completed {preset} runs on this cost model</p>
      ) : (
        <div className="table-wrap panel">
          <table className="table sticky-name">
            <thead>
              <tr>
                <th className="r">#</th>
                <th>Bot</th>
                <th className="r help" title="Completed backtests of this bot across every period, parameter set and cost model; the more variants tried, the more the best one owes to luck">
                  Trials
                </th>
                {COLUMNS.map((key) => (
                  <th
                    key={key}
                    className={`r sortable ${sort.key === key ? (sort.descending ? "desc" : "asc") : ""}`}
                    aria-sort={sort.key === key ? (sort.descending ? "descending" : "ascending") : "none"}
                  >
                    <button type="button" title={METRICS[key].title} onClick={() => toggle(key)}>
                      {METRICS[key].label}
                    </button>
                  </th>
                ))}
                <th>Finished</th>
              </tr>
            </thead>
            <tbody>
              {shown.map(({ entry, rank }) =>
                entry.kind === "benchmark" ? (
                  <tr key={`benchmark-${entry.benchmark.symbol}`} className="benchmark-row">
                    <td />
                    <td className="help" title={`Buy and hold ${entry.benchmark.symbol} over the same period and start cash`}>
                      {entry.benchmark.symbol} buy & hold
                    </td>
                    <td />
                    <MetricCells metrics={entry.benchmark.metrics} trading={false} />
                    <td />
                  </tr>
                ) : (
                  <tr key={entry.row.run_id}>
                    <td className="r muted">{rank}</td>
                    <td className="nowrap">
                      <a href={href({ page: "bot", name: entry.row.bot, run: entry.row.run_id })} title={paramsText(entry.row.params) || undefined}>
                        {entry.row.bot}
                      </a>
                      <RunBadges badges={entry.row.badges.filter((badge) => !shared.includes(badge))} short />
                    </td>
                    <td className="r num">{fmt.integer(entry.row.trials)}</td>
                    <MetricCells metrics={entry.row.metrics} trading />
                    <td className="nowrap muted">{fmt.local(entry.row.finished_at)}</td>
                  </tr>
                ),
              )}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
