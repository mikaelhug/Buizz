import { useState } from "react";
import type { Schemas } from "../api/client";
import { ABSENT, fmt, tone } from "../lib/format";
import type { Resource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";

type Trade = Schemas["TradeView"];
type SortKey = "closed" | "pnl" | "held";
type Sort = { key: SortKey; descending: boolean };

const SHOWN = 25;
const MINUTE_MS = 60_000;
const DAY_MS = 86_400_000;

const SORT_KEYS: Record<SortKey, { text: string; value: (trade: Trade) => number }> = {
  closed: { text: "Closed", value: (trade) => Date.parse(trade.closed_at) },
  held: { text: "Held", value: held },
  pnl: { text: "P&L", value: (trade) => Number(trade.pnl) },
};

function held(trade: Trade): number {
  return Date.parse(trade.closed_at) - Date.parse(trade.opened_at);
}

function duration(ms: number): string {
  if (ms < DAY_MS) {
    return `${fmt.integer(Math.round(ms / MINUTE_MS))}m`;
  }
  return `${fmt.integer(Math.round(ms / DAY_MS))}d`;
}

function median(values: number[]): number | undefined {
  const ordered = values.toSorted((a, b) => a - b);
  const middle = Math.floor(ordered.length / 2);
  return ordered.length === 0 ? undefined : ordered.length % 2 ? ordered[middle] : ((ordered[middle - 1] ?? 0) + (ordered[middle] ?? 0)) / 2;
}

function mean(values: number[]): number | undefined {
  return values.length === 0 ? undefined : values.reduce((sum, value) => sum + value, 0) / values.length;
}

function Summary({ trades }: { trades: Trade[] }) {
  const pnl = trades.map((trade) => Number(trade.pnl));
  const wins = pnl.filter((value) => value > 0);
  const losses = pnl.filter((value) => value < 0);
  const averageWin = mean(wins);
  const averageLoss = mean(losses);
  const hold = median(trades.map(held));
  const money = (label: string, title: string, value: number | undefined) => ({ label, title, text: fmt.money(value), tone: tone(value) });
  const stats = [
    money("Net P&L", "Sum over closed trades, after costs, dividends and borrow", pnl.reduce((sum, value) => sum + value, 0)),
    money("Expectancy", "Average P&L per closed trade", mean(pnl)),
    money("Avg win", "Average P&L of winning trades", averageWin),
    money("Avg loss", "Average P&L of losing trades", averageLoss),
    {
      label: "Payoff",
      title: "Average win divided by average loss",
      text: averageWin !== undefined && averageLoss !== undefined ? fmt.ratio(averageWin / -averageLoss) : ABSENT,
      tone: undefined,
    },
    money("Best", "Largest winning trade", wins.length ? Math.max(...wins) : undefined),
    money("Worst", "Largest losing trade", losses.length ? Math.min(...losses) : undefined),
    { label: "Median hold", title: "Median time from opening to closing a position", text: hold === undefined ? ABSENT : duration(hold), tone: undefined },
  ];
  return (
    <dl className="metric-grid trade-summary">
      {stats.map((stat) => (
        <div key={stat.label} className="metric">
          <dt className="help" title={stat.title}>
            {stat.label}
          </dt>
          <dd className={`num ${stat.tone ?? ""}`}>{stat.text}</dd>
        </div>
      ))}
    </dl>
  );
}

export function Trades({ trades }: { trades: Resource<Trade[]> }) {
  const [sort, setSort] = useState<Sort>({ key: "closed", descending: true });
  const [all, setAll] = useState(false);

  const toggle = (key: SortKey) => setSort((current) => ({ key, descending: current.key === key ? !current.descending : true }));
  const value = SORT_KEYS[sort.key].value;
  const ordered = (trades.data ?? []).toSorted((a, b) => (sort.descending ? value(b) - value(a) : value(a) - value(b)));
  const shown = all ? ordered : ordered.slice(0, SHOWN);

  return (
    <section className="panel">
      <h2>Trades</h2>
      <StaleFailure resource={trades} />
      {!trades.data ? (
        trades.error ? <Failure message={trades.error} onRetry={trades.reload} /> : <Loading shape="table" />
      ) : trades.data.length === 0 ? (
        <p className="empty">No closed trades</p>
      ) : (
        <>
          <Summary trades={trades.data} />
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Symbol</th>
                  <th>Side</th>
                  <th className="r help" title="Largest position held during the trade">
                    Size
                  </th>
                  <th>Opened</th>
                  {(["closed", "held", "pnl"] as const).map((key) => (
                    <th
                      key={key}
                      className={`sortable ${key === "closed" ? "" : "r"} ${sort.key === key ? (sort.descending ? "desc" : "asc") : ""}`}
                      aria-sort={sort.key === key ? (sort.descending ? "descending" : "ascending") : "none"}
                    >
                      <button type="button" onClick={() => toggle(key)}>
                        {SORT_KEYS[key].text}
                      </button>
                    </th>
                  ))}
                  <th>Entry reason</th>
                  <th>Exit reason</th>
                </tr>
              </thead>
              <tbody>
                {shown.map((trade) => (
                  <tr key={`${trade.symbol}-${trade.opened_at}-${trade.closed_at}`}>
                    <td className="sym">{trade.symbol}</td>
                    <td className={trade.qty > 0 ? "up" : "down"}>{trade.qty > 0 ? "long" : "short"}</td>
                    <td className="r num">{fmt.integer(Math.abs(trade.qty))}</td>
                    <td className="nowrap muted">{fmt.market(trade.opened_at)}</td>
                    <td className="nowrap muted">{fmt.market(trade.closed_at)}</td>
                    <td className="r num">{duration(held(trade))}</td>
                    <td className={`r num ${tone(Number(trade.pnl)) ?? ""}`}>{fmt.money(trade.pnl)}</td>
                    <td className="reason">{trade.opening_reason ?? ABSENT}</td>
                    <td className="reason">{trade.closing_reason ?? ABSENT}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {!all && ordered.length > SHOWN && (
            <button type="button" className="btn btn-sm load-more" onClick={() => setAll(true)}>
              Show all {fmt.integer(ordered.length)}
            </button>
          )}
        </>
      )}
    </section>
  );
}
