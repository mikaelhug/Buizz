import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { href } from "../lib/route";
import { appendPoint } from "../lib/series";
import { useResource } from "../lib/useResource";
import { Failure, Loading } from "../ui/Feedback";
import { HOVER_METRICS, METRICS, MetricValue } from "../ui/Metrics";
import { Sparkline } from "../ui/Sparkline";
import { StatusPill } from "../ui/Status";
import { BotActions } from "./BotActions";

type Bot = Schemas["BotSummary"];

export function BotGrid() {
  const bots = useResource(() => must(api.GET("/api/bots")), []);

  useSpineEvents((event) => {
    if (event.type === "run_status") {
      bots.reload();
    } else if (event.type === "equity") {
      const point = { at: event.at, equity: event.equity };
      bots.mutate((current) =>
        current.map((bot) =>
          bot.live_run === event.run_id ? { ...bot, equity: event.equity, sparkline: appendPoint(bot.sparkline, point) } : bot,
        ),
      );
    }
  });

  if (!bots.data) {
    return bots.error ? <Failure message={bots.error} onRetry={bots.reload} /> : <Loading />;
  }
  if (bots.data.length === 0) {
    return <p className="empty">No bots in bots/</p>;
  }
  return (
    <section className="bot-grid">
      {bots.data.map((bot) => (
        <BotCard key={bot.name} bot={bot} onChanged={bots.reload} />
      ))}
    </section>
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
        <span className="muted">{bot.last_run_at ? fmt.dateTime(bot.last_run_at) : "Never run"}</span>
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
