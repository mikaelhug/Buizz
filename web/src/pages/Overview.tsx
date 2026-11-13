import { type ReactNode, useState } from "react";
import { errorMessage, type Schemas } from "../api/client";
import { BENCHMARK, closeness, type Entry, excessCagr, type Glance, headline, variantOf, windowOf } from "../lib/bots";
import { fmt, tone } from "../lib/format";
import { href, navigate } from "../lib/route";
import { CHECKS } from "../ui/Gate";
import { Icon } from "../ui/Icon";
import { Annualized, firstClose, JUDGED_FROM_SESSION, type Live, LiveStatus, sessionsOf, Verdict } from "../ui/LiveTable";
import { type Mark, Sparkline } from "../ui/Sparkline";
import { SubPeriods } from "../ui/SubPeriods";
import { notify } from "../ui/Toaster";
import { goLive } from "./BotActions";
import { launchBacktest, launchSweep } from "./Launch";

type Job = Schemas["JobView"];

const SHOWN = 5;

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
    <section className="desk" aria-label="Overview">
      <LiveDesk entries={entries} marks={marks} />
      <div className="desk-rail">
        <AttentionPanel entries={entries} jobs={jobs} />
        <NextPanel entries={entries} />
      </div>
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

function PanelHead({ title, count, children }: { title: string; count?: number | undefined; children?: ReactNode }) {
  return (
    <header className="desk-head">
      <h2>
        {title}
        {count !== undefined && <span className="desk-count num">{count}</span>}
      </h2>
      {children && <span className="desk-aside">{children}</span>}
    </header>
  );
}

function LiveDesk({ entries, marks }: { entries: Entry[]; marks: Record<number, Mark> }) {
  const lives = entries.flatMap((entry) => (entry.live ? [entry.live] : []));
  const ordered = lives.toSorted((a, b) => (b.run.live_stats?.excess_cagr ?? -Infinity) - (a.run.live_stats?.excess_cagr ?? -Infinity));
  const shared = sharedSince(lives);
  const best = entries.filter((entry) => entry.group === "ready").toSorted((a, b) => (headline(b) ?? -Infinity) - (headline(a) ?? -Infinity))[0];
  const pending = lives.filter((live) => sessionsOf(live) === 0 && live.run.status === "running");
  const starts = pending[0] && firstClose(pending[0], true);

  return (
    <section className="desk-live" aria-label="Live paper">
      <PanelHead title="Live paper" count={lives.length}>
        {shared ? (
          <span className="num">
            {BENCHMARK} {fmt.change(shared.spy)} since {fmt.date(shared.since)}
          </span>
        ) : (
          starts && (
            <>
              First close <strong>{starts}</strong>; judged against its backtest from session {JUDGED_FROM_SESSION}, then ranked in the <a href={href({ page: "league" })}>League</a>
            </>
          )
        )}
      </PanelHead>
      {lives.length === 0 ? (
        <div className="desk-empty">
          <p>Nothing is live yet. Only live paper results can show a bot beats {BENCHMARK}.</p>
          {best?.bot.backtest && <GoLiveButton bot={best.bot} glance={best.bot.backtest} />}
        </div>
      ) : (
        <ul className="tiles">
          {ordered.map((live) => (
            <LiveTile key={live.run.id} live={live} defaults={entries.find((entry) => entry.live === live)?.bot.params ?? {}} mark={marks[live.run.id]} />
          ))}
        </ul>
      )}
    </section>
  );
}

function SessionTrack({ sessions }: { sessions: number }) {
  const filled = Math.min(sessions, JUDGED_FROM_SESSION);
  return (
    <span className="track" role="img" aria-label={`${fmt.integer(sessions)} of the ${JUDGED_FROM_SESSION} sessions before it is judged`}>
      {Array.from({ length: JUDGED_FROM_SESSION }, (_, index) => (
        <span key={index} className={index < filled ? "track-on" : undefined} />
      ))}
    </span>
  );
}

function LiveTile({ live, defaults, mark }: { live: Live; defaults: Record<string, unknown>; mark: Mark | undefined }) {
  const stats = live.run.live_stats;
  const started = stats !== null && stats !== undefined && stats.sessions > 0;
  const source = live.source;
  const variant = variantOf(live.run.config.params as Record<string, unknown>, defaults);
  const backtestExcess = excessCagr(source);
  return (
    <li className="tile">
      <div className="tile-top">
        <a className="tile-name" href={href({ page: "bot", name: live.run.bot, run: live.run.id })}>
          {live.run.bot}
        </a>
        {started && live.run.status !== "running" && <LiveStatus live={live} />}
      </div>
      {variant && <span className="tile-variant mono">{variant}</span>}
      <div className="tile-figure">
        {started ? (
          <>
            <span className="tile-number">
              <Annualized value={stats.excess_cagr} sessions={stats.sessions} />
            </span>
            <span className="tile-unit">/yr vs {BENCHMARK} live</span>
          </>
        ) : (
          <>
            <span className={`tile-number num ${tone(backtestExcess) ?? ""}`}>{fmt.change(backtestExcess)}</span>
            <span className="tile-unit">/yr vs {source?.benchmark_symbol ?? BENCHMARK} in {source ? windowOf(source) : "no backtest"}</span>
          </>
        )}
      </div>
      <div className={started ? "tile-chart" : "tile-chart tile-chart-backtest"}>
        {started ? (
          <Sparkline curve={stats.curve} mark={mark} benchmark={`total-return ${BENCHMARK}`} />
        ) : (
          source && <Sparkline curve={source.curve} mark={undefined} benchmark={`total-return ${source.benchmark_symbol}`} />
        )}
      </div>
      <div className="tile-foot">
        <SessionTrack sessions={sessionsOf(live)} />
        <Verdict live={live} />
      </div>
    </li>
  );
}

function checksText(glance: Glance): string {
  return glance.failed_checks.map((check) => CHECKS[check]).join("; ");
}

function NextPanel({ entries }: { entries: Entry[] }) {
  const ready = entries.filter((entry) => entry.group === "ready").toSorted((a, b) => (headline(b) ?? -Infinity) - (headline(a) ?? -Infinity));
  const closest = entries.filter((entry) => entry.group === "failing").toSorted((a, b) => closeness(a.bot.backtest) - closeness(b.bot.backtest));
  const untested = entries.filter((entry) => entry.group === "untested" && entry.bot.status !== "backtesting");
  const shown = ready.length > 0 ? ready : closest;

  return (
    <section className="desk-panel" aria-label="Next up">
      <PanelHead title={ready.length > 0 ? "Ready to go live" : "Closest to passing"} count={ready.length > 0 ? ready.length : undefined} />
      {shown.length === 0 ? (
        <div className="desk-empty">
          <p>Nothing is backtested on the current code yet. Backtest a bot on a 1y–10y preset to screen it against total-return {BENCHMARK}.</p>
          {untested[0] && (
            <button type="button" className="btn btn-sm" onClick={() => launchBacktest(untested[0]?.bot.name ?? "")}>
              Backtest {untested[0].bot.name}…
            </button>
          )}
        </div>
      ) : (
        <ul className="desk-rows">
          {shown.slice(0, SHOWN).map((entry) => {
            const glance = entry.bot.backtest;
            if (!glance) {
              return null;
            }
            const excess = excessCagr(glance);
            const passes = glance.failed_checks.length === 0;
            return (
              <li key={entry.key} className="desk-row">
                <a className="desk-row-name" href={href({ page: "bot", name: entry.bot.name, run: glance.run_id })}>
                  {entry.bot.name}
                </a>
                <span className={`num ${tone(excess) ?? ""}`}>{fmt.change(excess)}</span>
                <SubPeriods edges={glance.sub_periods} count={false} />
                <span className="desk-row-action">
                  {passes ? (
                    <GoLiveButton bot={entry.bot} glance={glance} />
                  ) : (
                    <button type="button" className="btn btn-xs" onClick={() => launchSweep(entry.bot.name)}>
                      Sweep…
                    </button>
                  )}
                </span>
                <span className="desk-row-detail">
                  {windowOf(glance)} · {passes ? "passes every check" : `fails ${checksText(glance)}`}
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

type Alert = { key: string; tone: "down" | "warn"; subject: string; what: string; detail: string; to: string };

function alertsOf(entries: Entry[], jobs: Job[] | undefined): Alert[] {
  const runs = entries.flatMap((entry): Alert[] => {
    const live = entry.live;
    const subject = entry.bot.name;
    if (live?.run.live_stats?.verdict === "falsified") {
      return [{ key: entry.key, tone: "down", subject, what: "off track", detail: "Live results left the range its backtest predicts; retiring it is your call.", to: href({ page: "bot", name: subject, run: live.run.id }) }];
    }
    if (live?.run.status === "paused" && live.run.message) {
      return [{ key: entry.key, tone: "warn", subject, what: "paused", detail: live.run.message, to: href({ page: "bot", name: subject, run: live.run.id }) }];
    }
    if (!live && entry.bot.status === "error") {
      return [{ key: entry.key, tone: "down", subject, what: "last run failed", detail: "Its latest run stopped with an error; open it from History for the bot's output.", to: href({ page: "bot", name: subject, tab: "runs" }) }];
    }
    if (!live && entry.bot.status === "stopped") {
      return [{ key: entry.key, tone: "warn", subject, what: "hit its drawdown stop", detail: "Its latest run fell by its drawdown limit, closed every position and ended.", to: href({ page: "bot", name: subject }) }];
    }
    return [];
  });
  const data = (jobs ?? []).flatMap((job): Alert[] =>
    job.last_outcome === "failed"
      ? [{ key: `job-${job.name}`, tone: "down", subject: `job ${job.name}`, what: "failed", detail: job.last_message ?? "Its last run failed.", to: href({ page: "data" }) }]
      : job.stale
        ? [{ key: `job-${job.name}`, tone: "warn", subject: `job ${job.name}`, what: "is late", detail: "It has not finished on schedule, so its data may be stale.", to: href({ page: "data" }) }]
        : [],
  );
  return [...runs, ...data];
}

type AlertGroup = { key: string; tone: Alert["tone"]; what: string; detail: string; alerts: Alert[] };

function grouped(alerts: Alert[]): AlertGroup[] {
  const groups = new Map<string, AlertGroup>();
  for (const alert of alerts) {
    const key = `${alert.tone}|${alert.what}|${alert.detail}`;
    const group = groups.get(key);
    if (group) {
      group.alerts.push(alert);
    } else {
      groups.set(key, { key, tone: alert.tone, what: alert.what, detail: alert.detail, alerts: [alert] });
    }
  }
  return [...groups.values()].toSorted((a, b) => Number(b.tone === "down") - Number(a.tone === "down"));
}

function Subjects({ alerts }: { alerts: Alert[] }) {
  const names = [...new Set(alerts.map((alert) => alert.subject))];
  const [only] = names;
  if (names.length === 1 && only !== undefined && alerts[0]) {
    return (
      <a href={alerts[0].to}>
        {only}
        {alerts.length > 1 && <span className="desk-times num"> ×{alerts.length}</span>}
      </a>
    );
  }
  return (
    <span>
      {alerts.map((alert, index) => (
        <span key={alert.key}>
          {index > 0 && ", "}
          <a href={alert.to}>{alert.subject}</a>
        </span>
      ))}
    </span>
  );
}

function AttentionPanel({ entries, jobs }: { entries: Entry[]; jobs: Job[] | undefined }) {
  const [all, setAll] = useState(false);
  const alerts = alertsOf(entries, jobs);
  const groups = grouped(alerts);
  const shown = all ? groups : groups.slice(0, SHOWN);
  return (
    <section className="desk-panel" aria-label="Needs attention">
      <PanelHead title="Needs attention" count={alerts.length} />
      {alerts.length === 0 ? (
        <p className="desk-empty desk-clear">
          <strong>All clear.</strong> No live run is off track, no latest run failed and every data job is on schedule.
        </p>
      ) : (
        <>
          <ul className="desk-rows">
            {shown.map((group) => (
              <li key={group.key} className={`desk-alert ${group.tone}`}>
                <span className="desk-alert-what">
                  <Subjects alerts={group.alerts} /> {group.what}
                </span>
                <span className="desk-row-detail">{group.detail}</span>
              </li>
            ))}
          </ul>
          {groups.length > SHOWN && (
            <button type="button" className="btn btn-xs btn-ghost desk-toggle" aria-expanded={all} onClick={() => setAll(!all)}>
              {all ? "Show fewer" : `Show all ${groups.length}`}
            </button>
          )}
        </>
      )}
    </section>
  );
}
