import { type MouseEvent, useId, useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { type SpineEvent, useSpineEvents } from "../api/stream";
import { fmt, label, tone } from "../lib/format";
import { href, navigate } from "../lib/route";
import { appendPoints } from "../lib/series";
import { useFrameBatch } from "../lib/useFrameBatch";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { Icon } from "../ui/Icon";
import { ChoiceMenu } from "../ui/Menu";
import { type MetricKey, METRICS, MetricValue } from "../ui/Metrics";
import { Sparkline } from "../ui/Sparkline";
import { StatusPill } from "../ui/Status";
import { notify } from "../ui/Toaster";
import { BacktestDialog } from "./BacktestDialog";
import { BotActions } from "./BotActions";
import "../styles/grid.css";

type Bot = Schemas["BotSummary"];
type EquityEvent = Extract<SpineEvent, { type: "equity" }>;

const SPARKLINE_CAP = 120;

type StatusFilter = "all" | Bot["status"];
type Order = "attention" | "name" | "recent" | "return";

const STATUS_FILTERS: readonly StatusFilter[] = ["all", "live", "paused", "backtesting", "stopped", "error", "idle"];
const ATTENTION: Record<Bot["status"], number> = { error: 0, stopped: 1, live: 2, paused: 3, backtesting: 4, idle: 5 };
const CARD_METRICS: readonly MetricKey[] = ["sharpe", "max_drawdown", "win_rate", "trades"];

const byName = (a: Bot, b: Bot) => a.name.localeCompare(b.name);
const returnOf = (bot: Bot) => bot.stats?.total_return ?? Number.NEGATIVE_INFINITY;

const ORDERS: Record<Order, { text: string; compare: (a: Bot, b: Bot) => number }> = {
  attention: { text: "Status", compare: (a, b) => ATTENTION[a.status] - ATTENTION[b.status] || byName(a, b) },
  return: { text: "Return", compare: (a, b) => returnOf(b) - returnOf(a) || byName(a, b) },
  recent: { text: "Last run", compare: (a, b) => (b.last_run_at ?? "").localeCompare(a.last_run_at ?? "") || byName(a, b) },
  name: { text: "Name", compare: byName },
};

const ORDER_CHOICES = (Object.keys(ORDERS) as Order[]).map((value) => ({ value, text: ORDERS[value].text }));

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
    return <p className="empty">No bots yet. Add a bot folder under bots/ and it shows up here.</p>;
  }
  const all = bots.data;
  const count = (option: StatusFilter) => (option === "all" ? all.length : all.filter((bot) => bot.status === option).length);
  const query = filter.trim().toLowerCase();
  const shown = all
    .filter((bot) => (status === "all" || bot.status === status) && bot.name.toLowerCase().includes(query))
    .toSorted(ORDERS[order].compare);
  return (
    <>
      <header className="page-head grid-head">
        <h1>Bots</h1>
        <div className="segmented segmented-sm status-filter" role="radiogroup" aria-label="Status">
          {STATUS_FILTERS.map((option) => {
            const total = count(option);
            return (
              <button
                key={option}
                type="button"
                role="radio"
                aria-checked={status === option}
                className={status === option ? "active" : undefined}
                disabled={total === 0 && status !== option}
                onClick={() => setStatus(option)}
              >
                {label(option)}{" "}
                <span className="count">{total}</span>
              </button>
            );
          })}
        </div>
        <div className="grid-tools">
          <input className="input input-sm" type="search" placeholder="Search bots" aria-label="Search bots" value={filter} onChange={(event) => setFilter(event.target.value)} />
          <ChoiceMenu label="Sort" value={order} choices={ORDER_CHOICES} onChange={setOrder} className="btn btn-sm sort-button">
            <span className="muted">Sort</span>
            {ORDERS[order].text}
            <Icon name="chevron" />
          </ChoiceMenu>
        </div>
      </header>
      <StaleFailure resource={bots} />
      {shown.length === 0 ? (
        <p className="empty">
          No bots match.{" "}
          <button
            type="button"
            className="btn btn-sm btn-ghost"
            onClick={() => {
              setFilter("");
              setStatus("all");
            }}
          >
            Clear filters
          </button>
        </p>
      ) : (
        <section className="bot-grid" aria-label="Bots">
          {shown.map((bot) => (
            <BotCard key={bot.name} bot={bot} onChanged={bots.reload} />
          ))}
        </section>
      )}
    </>
  );
}

const INTERACTIVE = "a, button, input, select, textarea, label, dialog, [role='dialog']";

function BotCard({ bot, onChanged }: { bot: Bot; onChanged: () => void }) {
  const route = { page: "bot", name: bot.name } as const;
  const open = (event: MouseEvent) => {
    const target = event.target as Element;
    if (!target.closest(INTERACTIVE) && !window.getSelection()?.toString()) {
      navigate(route);
    }
  };
  const ran = bot.last_run_at !== null && bot.last_run_at !== undefined;
  const headline = bot.stats?.total_return;
  const titleId = useId();
  return (
    <article className={`bot-card status-${bot.status}`} aria-labelledby={titleId} onClick={open}>
      <header className="bot-card-head">
        <h2 className="bot-name" id={titleId}>
          <a href={href(route)}>{bot.name}</a>
        </h2>
        <span className="tf">{bot.timeframe}</span>
        <StatusPill status={bot.status} />
      </header>
      {bot.description && <p className="bot-desc">{bot.description}</p>}
      {ran ? (
        <>
          <div className="bot-headline">
            <span className={`display num ${tone(headline) ?? ""}`}>{fmt.change(headline)}</span>
            <span className="bot-equity">
              <span className="muted">Equity</span> <span className="num">{fmt.money(bot.equity)}</span>
            </span>
            {bot.holdout_runs > 0 && <span className="badge badge-holdout">Holdout ×{bot.holdout_runs}</span>}
          </div>
          <Sparkline points={bot.sparkline} />
          <dl className="bot-stats">
            {CARD_METRICS.map((metric) => (
              <div key={metric}>
                <dt>{METRICS[metric].label}</dt>
                <dd>
                  <MetricValue metrics={bot.stats} metric={metric} />
                </dd>
              </div>
            ))}
          </dl>
          <footer className="bot-card-foot">
            <span className="muted">Last run {fmt.local(bot.last_run_at)}</span>
            <BotActions bot={bot} onChanged={onChanged} />
          </footer>
        </>
      ) : (
        <div className="bot-fresh">
          <p className="muted">No runs yet. Backtest it to see how it would have traded.</p>
          <span className="bot-fresh-actions">
            <BacktestButton bot={bot} />
            <BotActions bot={bot} onChanged={onChanged} />
          </span>
        </div>
      )}
    </article>
  );
}

function BacktestButton({ bot }: { bot: Bot }) {
  const [risk, setRisk] = useState<Schemas["RiskLimits"]>();
  const [busy, setBusy] = useState(false);

  const start = async () => {
    setBusy(true);
    try {
      const detail = await must(api.GET("/api/bots/{name}", { params: { path: { name: bot.name } } }));
      setRisk(detail.manifest.risk ?? {});
    } catch (error) {
      notify(`${bot.name}: ${errorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={() => void start()}>
        <Icon name="play" />
        Run a backtest
      </button>
      {risk && (
        <BacktestDialog
          bot={bot}
          risk={risk}
          onClose={() => setRisk(undefined)}
          onStarted={(run) => {
            setRisk(undefined);
            navigate({ page: "bot", name: bot.name, run: run.id });
          }}
        />
      )}
    </>
  );
}
