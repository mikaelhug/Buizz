import { useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { href, PRESETS, type Preset } from "../lib/route";
import { useResource } from "../lib/useResource";
import { Failure, Loading } from "../ui/Feedback";
import { type MetricKey, METRICS, MetricValue } from "../ui/Metrics";

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

const ASCENDING: ReadonlySet<MetricKey> = new Set(["volatility", "max_drawdown"]);

type Sort = { key: MetricKey; descending: boolean };
type Row = Schemas["LeaderboardRow"];

function sorted(rows: Row[], sort: Sort): Row[] {
  return rows.toSorted((a, b) => {
    const left = a.metrics[sort.key];
    const right = b.metrics[sort.key];
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

export function Leaderboard({ preset }: { preset: Preset }) {
  const board = useResource(() => must(api.GET("/api/leaderboard", { params: { query: { preset } } })), [preset]);
  const [sort, setSort] = useState<Sort>({ key: "sharpe", descending: true });

  useSpineEvents((event) => {
    if (event.type === "run_status" && event.status === "completed") {
      board.reload();
    }
  });

  const toggle = (key: MetricKey) =>
    setSort((current) => (current.key === key ? { key, descending: !current.descending } : { key, descending: !ASCENDING.has(key) }));

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
      </header>
      {!board.data ? (
        board.error ? <Failure message={board.error} onRetry={board.reload} /> : <Loading />
      ) : board.data.rows.length === 0 ? (
        <p className="empty">No completed {preset} runs on this cost model</p>
      ) : (
        <div className="table-wrap panel">
          <table className="table">
            <thead>
              <tr>
                <th className="r">#</th>
                <th>Bot</th>
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
              {sorted(board.data.rows, sort).map((row, index) => (
                <tr key={row.run_id}>
                  <td className="r muted">{index + 1}</td>
                  <td>
                    <a href={href({ page: "bot", name: row.bot, run: row.run_id })}>{row.bot}</a>
                  </td>
                  {COLUMNS.map((key) => (
                    <td key={key} className="r">
                      <MetricValue metrics={row.metrics} metric={key} />
                    </td>
                  ))}
                  <td className="nowrap muted">{fmt.local(row.finished_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </div>
  );
}
