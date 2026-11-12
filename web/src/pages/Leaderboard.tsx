import { useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { href, PRESETS, type Preset } from "../lib/route";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { FEW_TRADES, type MetricKey, METRICS, type Metrics, MetricValue, sharpeError } from "../ui/Metrics";
import { Info } from "../ui/Popover";
import { RunBadges } from "../ui/Status";
import "../styles/leaderboard.css";

type Column = MetricKey | "trials";

const GROUPS: readonly { name: string; about: string; columns: readonly Column[] }[] = [
  { name: "Returns", about: "What the run earned over the period, absolute and against the benchmark", columns: ["total_return", "cagr", "alpha"] },
  { name: "Risk", about: "How far and how often equity swung, and how much of it rode the market", columns: ["max_drawdown", "volatility", "beta", "exposure"] },
  {
    name: "Robustness",
    about: "Whether the result is likely to hold up: risk-adjusted return with its uncertainty, trade statistics and how many variants were tried",
    columns: ["sharpe", "sortino", "calmar", "trials", "win_rate", "profit_factor", "trades"],
  },
];

const ABOUT: Partial<Record<Column, string>> = {
  sortino: "Like Sharpe, but only downside volatility counts as risk",
  sharpe: "Annualised excess return over the Treasury rate per unit of volatility. The ± figure is one standard error over the run's sessions; two Sharpes within it of each other are not reliably different",
  trials: "Completed backtests of this bot across every period, parameter set and cost model; the more variants tried, the more the best one owes to luck",
};

const TRADE_METRICS: ReadonlySet<Column> = new Set(["win_rate", "profit_factor", "trades"]);

const ASCENDING: ReadonlySet<Column> = new Set(["volatility", "max_drawdown"]);

type Sort = { key: Column; descending: boolean };
type Row = Schemas["LeaderboardRow"];
type Entry = { kind: "bot"; row: Row } | { kind: "benchmark"; benchmark: Schemas["LeaderboardBenchmark"] };

function columnLabel(column: Column): string {
  return column === "trials" ? "Trials" : METRICS[column].label;
}

function columnAbout(column: Column): string | undefined {
  return ABOUT[column] ?? (column === "trials" ? undefined : METRICS[column].title);
}

function valueOf(entry: Entry, key: Column): number | null | undefined {
  if (key === "trials") {
    return entry.kind === "bot" ? entry.row.trials : undefined;
  }
  return (entry.kind === "bot" ? entry.row.metrics : entry.benchmark.metrics)[key];
}

function sorted(entries: Entry[], sort: Sort): Entry[] {
  return entries.toSorted((a, b) => {
    const left = valueOf(a, sort.key);
    const right = valueOf(b, sort.key);
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
  return `${cost.slippage_bps} bps slippage · ${cost.commission_per_share}/sh, min ${cost.commission_min} · ADV ${fmt.percent(cost.adv_participation)} · borrow ${fmt.percent(cost.borrow_rate)}`;
}

function SharpeCell({ metrics }: { metrics: Metrics }) {
  const error = sharpeError(metrics);
  return (
    <>
      <MetricValue metrics={metrics} metric="sharpe" />
      {error !== undefined && (
        <span className="se">
          <span aria-hidden="true">±</span>
          <span className="visually-hidden">plus or minus</span>
          {fmt.ratio(error)}
        </span>
      )}
    </>
  );
}

function cellContent(entry: Entry, key: Column) {
  if (entry.kind === "benchmark") {
    return key === "trials" || TRADE_METRICS.has(key) ? null : <MetricValue metrics={entry.benchmark.metrics} metric={key} />;
  }
  if (key === "trials") {
    return <span className="num">{fmt.integer(entry.row.trials)}</span>;
  }
  return key === "sharpe" ? <SharpeCell metrics={entry.row.metrics} /> : <MetricValue metrics={entry.row.metrics} metric={key} />;
}

function Cells({ entry }: { entry: Entry }) {
  const thinTrades = entry.kind === "bot" && entry.row.metrics.trades < FEW_TRADES;
  return GROUPS.flatMap((group) =>
    group.columns.map((key, index) => {
      const thin = thinTrades && TRADE_METRICS.has(key);
      return (
        <td key={key} className={`r${thin ? " thin" : ""}${index === 0 ? " group-start" : ""}`}>
          {cellContent(entry, key)}
          {thin && <span className="visually-hidden"> (few trades)</span>}
        </td>
      );
    }),
  );
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

  const toggle = (key: Column) =>
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
  const anyThin = rows.some((row) => row.metrics.trades < FEW_TRADES);

  return (
    <div className="leaderboard">
      <header className="page-head">
        <h1>Leaderboard</h1>
        <nav className="segmented" aria-label="Period">
          {PRESETS.map((option) => (
            <a key={option} href={href({ page: "leaderboard", preset: option })} className={option === preset ? "active" : undefined} aria-current={option === preset ? "page" : undefined}>
              {option}
            </a>
          ))}
        </nav>
        {rows.length > 0 && (
          <input className="input input-sm page-filter" type="search" placeholder="Search bots" aria-label="Search bots" value={filter} onChange={(event) => setFilter(event.target.value)} />
        )}
      </header>
      {board.data && (
        <dl className="facts board-facts">
          <div>
            <dt>Period</dt>
            <dd className="num">
              {board.data.period.from} → {board.data.period.to}
            </dd>
          </div>
          <div>
            <dt>
              Costs
              <Info label="Cost model">Every ranked run used the default cost model, so returns compare like for like across bots</Info>
            </dt>
            <dd>{costSummary(board.data.cost)}</dd>
          </div>
          {shared.length > 0 && (
            <div>
              <dt>Every run</dt>
              <dd className="badges-fact">
                <RunBadges badges={shared} />
              </dd>
            </div>
          )}
        </dl>
      )}
      <StaleFailure resource={board} />
      {!board.data ? (
        board.error ? <Failure message={board.error} onRetry={board.reload} /> : <Loading shape="table" />
      ) : board.data.rows.length === 0 ? (
        <div className="panel board-empty">
          <h2>No {preset} results yet</h2>
          <p className="muted">
            The leaderboard ranks each bot's latest completed {preset} backtest run with its default parameters and cost model.
          </p>
          <a className="btn btn-primary btn-sm" href={href({ page: "bots" })}>
            Run a {preset} backtest
          </a>
        </div>
      ) : (
        <>
          <div className="panel table-wrap board-wrap">
            <table className="table board">
              <thead>
                <tr className="board-groups">
                  <th className="board-name" rowSpan={2} scope="col">
                    Bot
                  </th>
                  {GROUPS.map((group) => (
                    <th key={group.name} colSpan={group.columns.length} scope="colgroup" className="group-start">
                      {group.name}
                      <Info label={group.name}>{group.about}</Info>
                    </th>
                  ))}
                  <th rowSpan={2} scope="col" className="group-start">
                    Finished
                  </th>
                </tr>
                <tr className="board-columns">
                  {GROUPS.flatMap((group) =>
                    group.columns.map((key, index) => {
                      const about = columnAbout(key);
                      const active = sort.key === key;
                      return (
                        <th
                          key={key}
                          scope="col"
                          className={`r sortable${active ? (sort.descending ? " desc" : " asc") : ""}${index === 0 ? " group-start" : ""}`}
                          aria-sort={active ? (sort.descending ? "descending" : "ascending") : "none"}
                        >
                          <span className="th-inner">
                            {about && <Info label={columnLabel(key)}>{about}</Info>}
                            <button type="button" onClick={() => toggle(key)}>
                              {columnLabel(key)}
                            </button>
                          </span>
                        </th>
                      );
                    }),
                  )}
                </tr>
              </thead>
              <tbody>
                {shown.map(({ entry, rank }) =>
                  entry.kind === "benchmark" ? (
                    <tr key={`benchmark-${entry.benchmark.symbol}`} className="benchmark-row">
                      <th scope="row" className="board-name">
                        <span className="rank" />
                        {entry.benchmark.symbol} buy & hold
                        <Info label={`${entry.benchmark.symbol} buy & hold`}>
                          Buying {entry.benchmark.symbol} at the start and holding it, over the same period and with the same start cash
                        </Info>
                      </th>
                      <Cells entry={entry} />
                      <td className="group-start" />
                    </tr>
                  ) : (
                    <tr key={entry.row.run_id}>
                      <th scope="row" className="board-name">
                        <span className="nowrap">
                          <span className="rank num">{rank}</span>
                          <a href={href({ page: "bot", name: entry.row.bot, run: entry.row.run_id })}>{entry.row.bot}</a>
                        </span>
                        <RunBadges badges={entry.row.badges.filter((badge) => !shared.includes(badge))} />
                      </th>
                      <Cells entry={entry} />
                      <td className="nowrap muted group-start">{fmt.local(entry.row.finished_at)}</td>
                    </tr>
                  ),
                )}
              </tbody>
            </table>
          </div>
          {anyThin && <p className="board-note muted">Greyed trade statistics come from fewer than {FEW_TRADES} closed trades and say little on their own.</p>}
        </>
      )}
    </div>
  );
}
