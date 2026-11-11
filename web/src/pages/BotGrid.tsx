import { api, must, type Schemas } from "../api/client";
import { type SpineEvent, useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { href } from "../lib/route";
import { appendPoints } from "../lib/series";
import { useFrameBatch } from "../lib/useFrameBatch";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { HOVER_METRICS, METRICS, MetricValue } from "../ui/Metrics";
import { Sparkline } from "../ui/Sparkline";
import { StatusPill } from "../ui/Status";
import { BotActions } from "./BotActions";

type Bot = Schemas["BotSummary"];
type EquityEvent = Extract<SpineEvent, { type: "equity" }>;

const SPARKLINE_CAP = 120;

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

  if (!bots.data) {
    return bots.error ? <Failure message={bots.error} onRetry={bots.reload} /> : <Loading />;
  }
  if (bots.data.length === 0) {
    return <p className="empty">No bots in bots/</p>;
  }
  return (
    <>
      <StaleFailure resource={bots} />
      <section className="bot-grid">
      {bots.data.map((bot) => (
        <BotCard key={bot.name} bot={bot} onChanged={bots.reload} />
        ))}
      </section>
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
