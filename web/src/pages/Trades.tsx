import { useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { ABSENT, fmt, tone } from "../lib/format";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { type Stat, StatGrid } from "../ui/Metrics";
import { Info } from "../ui/Popover";
import { SymbolLink } from "../ui/SymbolChart";
import { matches, moveBetweenRows, openRow, SortHeader, type SortColumns, TableFilter, useFlash, useSort } from "../ui/Table";

type Trade = Schemas["TradeView"];
type Column = "symbol" | "side" | "size" | "opened" | "closed" | "held" | "pnl" | "entry" | "exit";

const SHOWN = 25;
const MINUTE_MS = 60_000;
const DAY_MS = 86_400_000;

const COLUMNS: SortColumns<Trade, Column> = {
  symbol: { value: (trade) => trade.symbol },
  side: { value: side },
  size: { value: (trade) => Math.abs(trade.qty), descending: true },
  opened: { value: (trade) => Date.parse(trade.opened_at), descending: true },
  closed: { value: (trade) => Date.parse(trade.closed_at), descending: true },
  held: { value: held, descending: true },
  pnl: { value: (trade) => Number(trade.pnl), descending: true },
  entry: { value: (trade) => trade.opening_reason },
  exit: { value: (trade) => trade.closing_reason },
};

function side(trade: Trade): string {
  return trade.qty > 0 ? "long" : "short";
}

function tradeKey(trade: Trade): string {
  return `${trade.symbol}-${trade.opened_at}-${trade.closed_at}`;
}

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
  const money = (label: string, title: string, value: number | undefined): Stat => ({ label, title, text: fmt.money(value), tone: tone(value) });
  const stats: Stat[] = [
    money("Net P&L", "Sum over closed trades, after costs, dividends and borrow", pnl.reduce((sum, value) => sum + value, 0)),
    money("Expectancy", "Average P&L per closed trade", mean(pnl)),
    money("Avg win", "Average P&L of winning trades", averageWin),
    money("Avg loss", "Average P&L of losing trades", averageLoss),
    {
      label: "Payoff",
      title: "Average win divided by average loss",
      text: averageWin !== undefined && averageLoss !== undefined ? fmt.ratio(averageWin / -averageLoss) : ABSENT,
      note: averageWin === undefined ? "no winning trades" : averageLoss === undefined ? "no losing trades" : undefined,
    },
    money("Best", "Largest winning trade", wins.length ? Math.max(...wins) : undefined),
    money("Worst", "Largest losing trade", losses.length ? Math.min(...losses) : undefined),
    { label: "Median hold", title: "Median time from opening to closing a position", text: hold === undefined ? ABSENT : duration(hold) },
  ];
  return <StatGrid stats={stats} />;
}

type Props = { runId: number; positionsHref: string; symbolHref: (symbol: string) => string };

export function Trades({ runId, positionsHref, symbolHref }: Props) {
  const trades = useResource(() => must(api.GET("/api/runs/{id}/trades", { params: { path: { id: runId } } })), [runId]);
  useSpineEvents((event) => {
    if (event.type === "fill" && event.run_id === runId) {
      trades.reload();
    }
  });
  const sorting = useSort("trades", COLUMNS, { key: "closed", descending: true });
  const flash = useFlash(trades.data, tradeKey);
  const [query, setQuery] = useState("");
  const [all, setAll] = useState(false);

  const matching = sorting.order((trades.data ?? []).filter((trade) => matches(query, trade.symbol, side(trade), trade.opening_reason, trade.closing_reason)));
  const shown = all ? matching : matching.slice(0, SHOWN);
  const net = matching.reduce((sum, trade) => sum + Number(trade.pnl), 0);

  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Trades</h2>
        {trades.data && trades.data.length > 0 && (
          <>
            <span className="muted">{fmt.integer(trades.data.length)} closed</span>
            <TableFilter label="Filter trades" value={query} onChange={setQuery} />
          </>
        )}
      </header>
      <StaleFailure resource={trades} />
      {!trades.data ? (
        trades.error ? <Failure message={trades.error} onRetry={trades.reload} /> : <Loading shape="table" />
      ) : trades.data.length === 0 ? (
        <p className="empty">
          No closed trades yet. A trade is a position opened and taken back to flat; open ones are under <a href={positionsHref}>Positions &amp; events</a>.
        </p>
      ) : (
        <>
          <Summary trades={trades.data} />
          {matching.length === 0 ? (
            <p className="empty">No trades match “{query.trim()}”.</p>
          ) : (
            <div className="table-wrap table-scroll">
              <table className="table fin">
                <thead>
                  <tr>
                    <SortHeader sorting={sorting} column="symbol">
                      Symbol
                    </SortHeader>
                    <SortHeader sorting={sorting} column="side">
                      Side
                    </SortHeader>
                    <SortHeader sorting={sorting} column="size" numeric info={<Info label="Size">Largest position held during the trade, in shares.</Info>}>
                      Size
                    </SortHeader>
                    <SortHeader sorting={sorting} column="opened">
                      Opened
                    </SortHeader>
                    <SortHeader sorting={sorting} column="closed">
                      Closed
                    </SortHeader>
                    <SortHeader sorting={sorting} column="held" numeric>
                      Held
                    </SortHeader>
                    <SortHeader sorting={sorting} column="pnl" numeric>
                      P&amp;L
                    </SortHeader>
                    <SortHeader sorting={sorting} column="entry">
                      Entry reason
                    </SortHeader>
                    <SortHeader sorting={sorting} column="exit">
                      Exit reason
                    </SortHeader>
                  </tr>
                </thead>
                <tbody onKeyDown={moveBetweenRows}>
                  {shown.map((trade) => (
                    <tr key={tradeKey(trade)} className="clickable" onClick={openRow(symbolHref(trade.symbol))} {...flash.row(tradeKey(trade))}>
                      <td className="sym">
                        <SymbolLink symbol={trade.symbol} href={symbolHref} />
                      </td>
                      <td className={trade.qty > 0 ? "up" : "down"}>{side(trade)}</td>
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
                <tfoot>
                  <tr>
                    <th scope="row" colSpan={6}>
                      {query.trim() ? `${fmt.integer(matching.length)} matching trades` : `All ${fmt.integer(matching.length)} trades`}
                    </th>
                    <td className={`r num ${tone(net) ?? ""}`}>{fmt.money(net)}</td>
                    <td colSpan={2} />
                  </tr>
                </tfoot>
              </table>
            </div>
          )}
          {!all && matching.length > SHOWN && (
            <div className="load-more">
              <button type="button" className="btn btn-sm" onClick={() => setAll(true)}>
                Show all {fmt.integer(matching.length)}
              </button>
            </div>
          )}
        </>
      )}
    </section>
  );
}
