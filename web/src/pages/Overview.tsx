import { type ReactNode, useState } from "react";
import { errorMessage, type Schemas } from "../api/client";
import { BENCHMARK, closeness, type Entry, excessCagr, type Glance, headline, variantOf, windowOf } from "../lib/bots";
import { fmt, tone } from "../lib/format";
import { href, navigate } from "../lib/route";
import { CHECKS } from "../ui/Gate";
import { Icon } from "../ui/Icon";
import { Annualized, firstClose, JUDGED_FROM_SESSION, type Live, sessionsOf, Verdict } from "../ui/LiveTable";
import { type Mark, Sparkline } from "../ui/Sparkline";
import { sourceSentence } from "../ui/Source";
import { SubPeriods } from "../ui/SubPeriods";
import { notify } from "../ui/Toaster";
import { goLive } from "./BotActions";
import { launchBacktest, launchSweep } from "./Launch";

type Job = Schemas["JobView"];

const SHOWN = 4;

export function GoLiveButton({ bot, glance, primary = true }: { bot: Entry["bot"]; glance: Glance; primary?: boolean }) {
  const [busy, setBusy] = useState(false);
  const start = () => {
    setBusy(true);
    goLive(bot.name, glance.run_id).then(
      (run) => navigate({ page: "bot", name: bot.name, run: run.id }),
      (error: unknown) => {
        notify(`${bot.name}: ${errorMessage(error)}`);
        setBusy(false);
      },
    );
  };
  return (
    <button type="button" className={primary ? "btn btn-sm btn-primary" : "btn btn-sm"} disabled={busy} onClick={start} aria-label={`Go live with ${bot.name} from backtest #${glance.run_id}`}>
      <Icon name="play" />
      Go live
    </button>
  );
}

export function Overview({ entries, jobs, marks }: { entries: Entry[]; jobs: Job[] | undefined; marks: Record<number, Mark> }) {
  return (
    <section className="overview-cards" aria-label="Overview">
      <LiveCard entries={entries} marks={marks} />
      <NextCard entries={entries} />
      <AttentionCard entries={entries} jobs={jobs} />
    </section>
  );
}

function sharedSince(lives: Live[]): { since: string; spy: number } | undefined {
  const started = lives.filter((live) => sessionsOf(live) > 0);
  const [first] = started;
  const stats = first?.run.live_stats;
  if (!first || !stats || started.some((live) => live.run.config.start !== first.run.config.start || live.run.live_stats?.through !== stats.through)) {
    return undefined;
  }
  return { since: first.run.config.start, spy: stats.benchmark_return };
}

function OverviewCard({ title, count, aside, label, children }: { title: string; count?: number | undefined; aside?: ReactNode; label: string; children: ReactNode }) {
  return (
    <section className="ov-card" aria-label={label}>
      <header className="ov-head">
        <h2>
          {title}
          {count !== undefined && <span className="ov-count num">{count}</span>}
        </h2>
        {aside && <span className="ov-aside">{aside}</span>}
      </header>
      {children}
    </section>
  );
}

function LiveCard({ entries, marks }: { entries: Entry[]; marks: Record<number, Mark> }) {
  const lives = entries.flatMap((entry) => (entry.live ? [entry.live] : []));
  const ordered = lives.toSorted((a, b) => (b.run.live_stats?.excess_cagr ?? -Infinity) - (a.run.live_stats?.excess_cagr ?? -Infinity));
  const shared = sharedSince(lives);
  const best = entries.filter((entry) => entry.group === "ready").toSorted((a, b) => (headline(b) ?? -Infinity) - (headline(a) ?? -Infinity))[0];
  const pending = lives.filter((live) => sessionsOf(live) === 0);
  const starts = pending[0] && firstClose(pending[0], true);

  return (
    <OverviewCard
      title="Live paper"
      count={lives.length}
      label="Live paper"
      aside={shared && <span className="num">{BENCHMARK} {fmt.change(shared.spy)} since {fmt.date(shared.since)}</span>}
    >
      {lives.length === 0 ? (
        <div className="ov-empty">
          <p>Nothing is live yet. Only live paper results can show a bot beats {BENCHMARK}.</p>
          {best?.bot.backtest && <GoLiveButton bot={best.bot} glance={best.bot.backtest} />}
        </div>
      ) : (
        <>
          {pending.length === lives.length && (
            <p className="ov-lede">
              {lives.length === 1 ? "It trades" : "They trade"} for the first time at the close of <strong>{starts ?? "the next session"}</strong>. Each close marks equity against total-return{" "}
              {BENCHMARK}; from session {JUDGED_FROM_SESSION} each run is judged against the backtest it came from and ranked in the <a href={href({ page: "league" })}>League</a>.
            </p>
          )}
          <ul className="ov-list">
            {ordered.slice(0, SHOWN).map((live) => (
              <LiveLine key={live.run.id} live={live} defaults={entries.find((entry) => entry.live === live)?.bot.params ?? {}} mark={marks[live.run.id]} />
            ))}
          </ul>
          {ordered.length > SHOWN && <p className="ov-more muted">and {ordered.length - SHOWN} more in the list below</p>}
        </>
      )}
    </OverviewCard>
  );
}

function LiveLine({ live, defaults, mark }: { live: Live; defaults: Record<string, unknown>; mark: Mark | undefined }) {
  const stats = live.run.live_stats;
  const started = stats && stats.sessions > 0;
  const variant = variantOf(live.run.config.params as Record<string, unknown>, defaults);
  return (
    <li className="ov-line">
      <div className="ov-who">
        <a className="ov-name" href={href({ page: "bot", name: live.run.bot, run: live.run.id })}>
          {live.run.bot}
        </a>
        {variant && <span className="ov-variant mono">{variant}</span>}
      </div>
      <div className="ov-figure">
        {started && (
          <span className="ov-number">
            <Annualized value={stats.excess_cagr} sessions={stats.sessions} />
            <span className="ov-unit">/yr vs {BENCHMARK}</span>
          </span>
        )}
        <Verdict live={live} />
      </div>
      <p className="ov-sub">{sourceSentence(live)}</p>
      {started && (
        <div className="ov-spark">
          <Sparkline curve={stats.curve} mark={mark} benchmark={`total-return ${BENCHMARK}`} />
        </div>
      )}
    </li>
  );
}

function checksText(glance: Glance): string {
  return glance.failed_checks.map((check) => CHECKS[check]).join("; ");
}

function NextCard({ entries }: { entries: Entry[] }) {
  const ready = entries.filter((entry) => entry.group === "ready").toSorted((a, b) => (headline(b) ?? -Infinity) - (headline(a) ?? -Infinity));
  const closest = entries.filter((entry) => entry.group === "failing").toSorted((a, b) => closeness(a.bot.backtest) - closeness(b.bot.backtest));
  const untested = entries.filter((entry) => entry.group === "untested" && entry.bot.status !== "backtesting");
  const shown = ready.length > 0 ? ready : closest;

  return (
    <OverviewCard title={ready.length > 0 ? "Ready to go live" : "Closest to passing"} count={ready.length > 0 ? ready.length : undefined} label="Next up">
      {shown.length === 0 ? (
        <div className="ov-empty">
          <p>Nothing is backtested on the current code yet. Backtest a bot on a 1y–10y preset to screen it against total-return {BENCHMARK}.</p>
          {untested[0] && (
            <button type="button" className="btn btn-sm" onClick={() => launchBacktest(untested[0]?.bot.name ?? "")}>
              Backtest {untested[0].bot.name}…
            </button>
          )}
        </div>
      ) : (
        <>
          <p className="ov-lede">
            {ready.length > 0
              ? "These passed the backtest gate. Going live is the only way to learn whether the edge is real."
              : "No bot passes the backtest gate yet. These fail the fewest checks; a sweep may find a variant that passes."}
          </p>
          <ul className="ov-list">
            {shown.slice(0, SHOWN - 1).map((entry) => {
              const glance = entry.bot.backtest;
              if (!glance) {
                return null;
              }
              const excess = excessCagr(glance);
              return (
                <li key={entry.key} className="ov-line">
                  <div className="ov-who">
                    <a className="ov-name" href={href({ page: "bot", name: entry.bot.name, run: glance.run_id })}>
                      {entry.bot.name}
                    </a>
                    <span className="ov-variant muted">{windowOf(glance)}</span>
                  </div>
                  <div className="ov-figure">
                    <span className={`ov-number num ${tone(excess) ?? ""}`}>
                      {fmt.change(excess)}
                      <span className="ov-unit">/yr vs {glance.benchmark_symbol}</span>
                    </span>
                    <SubPeriods edges={glance.sub_periods} />
                  </div>
                  <p className="ov-sub">{glance.failed_checks.length === 0 ? "Passes every check of the gate." : `Fails: ${checksText(glance)}.`}</p>
                  <div className="ov-action">
                    {glance.failed_checks.length === 0 ? (
                      <GoLiveButton bot={entry.bot} glance={glance} />
                    ) : (
                      <button type="button" className="btn btn-sm" onClick={() => launchSweep(entry.bot.name)}>
                        Sweep…
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        </>
      )}
    </OverviewCard>
  );
}

type Alert = { key: string; tone: "down" | "warn"; text: string; detail: string; to: string };

function alertsOf(entries: Entry[], jobs: Job[] | undefined): Alert[] {
  const runs = entries.flatMap((entry): Alert[] => {
    const live = entry.live;
    if (live?.run.live_stats?.verdict === "falsified") {
      return [{ key: entry.key, tone: "down", text: `${entry.bot.name} is off track`, detail: "Live results left the range its backtest predicts; retiring it is your call.", to: href({ page: "bot", name: entry.bot.name, run: live.run.id }) }];
    }
    if (live?.run.status === "paused" && live.run.message) {
      return [{ key: entry.key, tone: "warn", text: `${entry.bot.name} paused`, detail: live.run.message, to: href({ page: "bot", name: entry.bot.name, run: live.run.id }) }];
    }
    if (!live && entry.bot.status === "error") {
      return [{ key: entry.key, tone: "down", text: `${entry.bot.name}: last run failed`, detail: "Its latest run stopped with an error; open it from History for the bot's output.", to: href({ page: "bot", name: entry.bot.name, tab: "runs" }) }];
    }
    if (!live && entry.bot.status === "stopped") {
      return [{ key: entry.key, tone: "warn", text: `${entry.bot.name} hit its drawdown stop`, detail: "Its latest run fell by its drawdown limit, closed every position and ended.", to: href({ page: "bot", name: entry.bot.name }) }];
    }
    return [];
  });
  const data = (jobs ?? []).flatMap((job): Alert[] =>
    job.last_outcome === "failed"
      ? [{ key: `job-${job.name}`, tone: "down", text: `Data job ${job.name} failed`, detail: job.last_message ?? "Its last run failed.", to: href({ page: "data" }) }]
      : job.stale
        ? [{ key: `job-${job.name}`, tone: "warn", text: `Data job ${job.name} is late`, detail: "It has not finished on schedule, so its data may be stale.", to: href({ page: "data" }) }]
        : [],
  );
  return [...runs, ...data];
}

function AttentionCard({ entries, jobs }: { entries: Entry[]; jobs: Job[] | undefined }) {
  const [all, setAll] = useState(false);
  const alerts = alertsOf(entries, jobs);
  const shown = all ? alerts : alerts.slice(0, SHOWN);
  return (
    <OverviewCard title="Needs attention" count={alerts.length} label="Needs attention">
      {alerts.length === 0 ? (
        <div className="ov-empty ov-clear">
          <p>
            <strong>All clear.</strong> No live run is off track, no latest run failed and every data job is on schedule.
          </p>
        </div>
      ) : (
        <>
          <ul className="ov-alerts">
            {shown.map((alert) => (
              <li key={alert.key} className={`ov-alert ${alert.tone}`}>
                <a href={alert.to}>{alert.text}</a>
                <span className="ov-alert-detail">{alert.detail}</span>
              </li>
            ))}
          </ul>
          {alerts.length > SHOWN && (
            <button type="button" className="btn btn-xs btn-ghost ov-toggle" aria-expanded={all} onClick={() => setAll(!all)}>
              {all ? "Show fewer" : `Show all ${alerts.length}`}
            </button>
          )}
        </>
      )}
    </OverviewCard>
  );
}
