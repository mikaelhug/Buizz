import { useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { type SpineEvent, useSpineEvents } from "../api/stream";
import { fmt, label } from "../lib/format";
import { href } from "../lib/route";
import { appendPoints } from "../lib/series";
import { useFrameBatch } from "../lib/useFrameBatch";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { HOVER_METRICS, METRICS, MetricValue } from "../ui/Metrics";
import { Sparkline } from "../ui/Sparkline";
import { StatusPill } from "../ui/Status";
import { BotActions } from "./BotActions";
import "../styles/grid.css";

type Bot = Schemas["BotSummary"];
type EquityEvent = Extract<SpineEvent, { type: "equity" }>;

const SPARKLINE_CAP = 120;

type StatusFilter = "all" | Bot["status"];
type Order = "attention" | "name" | "recent";

const STATUS_FILTERS: readonly StatusFilter[] = ["all", "error", "stopped", "live", "paused", "backtesting", "idle"];
const ATTENTION: Record<Bot["status"], number> = { error: 0, stopped: 1, live: 2, paused: 3, backtesting: 4, idle: 5 };

const byName = (a: Bot, b: Bot) => a.name.localeCompare(b.name);

const ORDERS: Record<Order, { text: string; compare: (a: Bot, b: Bot) => number }> = {
  attention: { text: "By status", compare: (a, b) => ATTENTION[a.status] - ATTENTION[b.status] || byName(a, b) },
  name: { text: "By name", compare: byName },
  recent: { text: "By last run", compare: (a, b) => (b.last_run_at ?? "").localeCompare(a.last_run_at ?? "") || byName(a, b) },
};

function rebucket<P>(points: P[]): P[] {
  return points.length <= SPARKLINE_CAP ? points : points.filter((_, index) => index % 2 === 0 || index === points.length - 1);
}

function withEquity(bot: Bot, events: EquityEvent[]): Bot {
  const mine = events.filter((event) => event.run_id === bot.live_run);
  const last = mine.at(-1);
  if (!last) {
    return bot;
  }
  const points = mine.map((event) => ({ at: event.at, equity: event.equity }));
  return { ...bot, equity: last.equity, sparkline: rebucket(appendPoints(bot.sparkline, points)) };
}

export function BotGrid() {
  const bots = useResource(() => must(api.GET("/api/bots")), []);

  const applyEquity = useFrameBatch<EquityEvent>((events) =>
    bots.mutate((current) => {
      const next = current.map((bot) => withEquity(bot, events));
      return next.some((bot, index) => bot !== current[index]) ? next : current;
    }),
  );

  useSpineEvents((event) => {
    if (event.type === "run_status") {
      bots.reload();
    } else if (event.type === "equity" && bots.data?.some((bot) => bot.live_run === event.run_id)) {
      applyEquity(event);
    }
  });

  const [filter, setFilter] = useState("");
  const [status, setStatus] = useState<StatusFilter>("all");
  const [order, setOrder] = useState<Order>("attention");

  if (!bots.data) {
    return bots.error ? <Failure message={bots.error} onRetry={bots.reload} /> : <Loading shape="cards" />;
  }
  if (bots.data.length === 0) {
    return <p className="empty">No bots in bots/</p>;
  }
  const counts = new Map<StatusFilter, number>([["all", bots.data.length]]);
  for (const bot of bots.data) {
    counts.set(bot.status, (counts.get(bot.status) ?? 0) + 1);
  }
  const query = filter.trim().toLowerCase();
  const shown = bots.data
    .filter((bot) => (status === "all" || bot.status === status) && bot.name.toLowerCase().includes(query))
    .toSorted(ORDERS[order].compare);
  return (
    <>
      <header className="page-head">
        <div className="segmented segmented-sm" role="radiogroup" aria-label="Status">
          {STATUS_FILTERS.filter((option) => counts.has(option)).map((option) => (
            <button key={option} type="button" role="radio" aria-checked={status === option} className={status === option ? "active" : undefined} onClick={() => setStatus(option)}>
              {label(option)} {counts.get(option)}
            </button>
          ))}
        </div>
        <select className="input input-sm" aria-label="Sort" value={order} onChange={(event) => setOrder(event.target.value as Order)}>
          {(Object.keys(ORDERS) as Order[]).map((option) => (
            <option key={option} value={option}>
              {ORDERS[option].text}
            </option>
          ))}
        </select>
        <input className="input input-sm page-filter" type="search" placeholder="Filter bots" aria-label="Filter bots" value={filter} onChange={(event) => setFilter(event.target.value)} />
      </header>
      <StaleFailure resource={bots} />
      {shown.length === 0 ? (
        <p className="empty">No matching bots</p>
      ) : (
        <section className="bot-grid">
          {shown.map((bot) => (
            <BotCard key={bot.name} bot={bot} onChanged={bots.reload} />
          ))}
        </section>
      )}
    </>
  );
}

function BotCard({ bot, onChanged }: { bot: Bot; onChanged: () => void }) {
  return (
    <article className={`card bot-card status-${bot.status}`}>
      <header className="bot-card-head">
        <a className="bot-name stretched" href={href({ page: "bot", name: bot.name })} title={bot.description}>
          {bot.name}
        </a>
        <span className="tf">{bot.timeframe}</span>
        <StatusPill status={bot.status} />
      </header>
      <div className="bot-card-body">
        <div className="bot-equity">
          <span className="num">{fmt.money(bot.equity)}</span>
          {bot.holdout_runs > 0 && (
            <span className="badge badge-holdout help" title="Runs on the locked holdout period">
              Holdout ×{bot.holdout_runs}
            </span>
          )}
        </div>
        <Sparkline points={bot.sparkline} />
      </div>
      <footer className="bot-card-foot">
        <span className="muted">{bot.last_run_at ? fmt.local(bot.last_run_at) : "Never run"}</span>
        <BotActions bot={bot} onChanged={onChanged} />
      </footer>
      {bot.stats && (
        <dl className="hover-stats">
          {HOVER_METRICS.map((metric) => (
            <div key={metric}>
              <dt>{METRICS[metric].label}</dt>
              <dd>
                <MetricValue metrics={bot.stats} metric={metric} />
              </dd>
            </div>
          ))}
        </dl>
      )}
    </article>
  );
}
