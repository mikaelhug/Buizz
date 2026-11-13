import { useRef, useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { type SpineEvent, useSpineEvents } from "../api/stream";
import { BENCHMARK, type Bot, closeness, type Entry, entriesOf, excessCagr, type Glance, type Group, GROUP_TEXT, GROUPS, headline, variantOf, windowOf } from "../lib/bots";
import { fmt, tone } from "../lib/format";
import { href, navigate, useQueryParam } from "../lib/route";
import { useStored } from "../lib/stored";
import { useFrameBatch } from "../lib/useFrameBatch";
import { useResource } from "../lib/useResource";
import { ExcessBar, excessScale } from "../ui/ExcessBar";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { GateResult } from "../ui/Gate";
import { Icon } from "../ui/Icon";
import { EmptyState } from "../ui/Kit";
import { Annualized, type Live, Verdict } from "../ui/LiveTable";
import { type MenuAction, OverflowMenu } from "../ui/Menu";
import { Info } from "../ui/Popover";
import { BulkBar, CompareButton, SelectAll, SelectBox, useSelection } from "../ui/Selection";
import { SourceBacktest } from "../ui/Source";
import { type Mark, Sparkline } from "../ui/Sparkline";
import { StatusPill } from "../ui/Status";
import { SubPeriods } from "../ui/SubPeriods";
import { Ago, matches, moveBetweenRows, PageSearch, SortHeader, type SortColumns, useSort } from "../ui/Table";
import { notify } from "../ui/Toaster";
import { goLive, isRunning, pauseRun, resumeRun, undoPause, useLiveControls } from "./BotActions";
import { launchBacktest, launchSweep } from "./Launch";
import { GoLiveButton, Overview } from "./Overview";
import { type Preset, SWEEP_PRESETS } from "./SweepDialog";
import "../styles/board.css";
import "../styles/bots.css";

type Run = Schemas["RunSummary"];
type EquityEvent = Extract<SpineEvent, { type: "equity" }>;

const HEADLINE_ABOUT =
  "The bot's CAGR minus total-return SPY's (dividends reinvested) over exactly the same sessions: live since the run started when it is live, else its screening backtest. SPY's own figures for each window are stated once above the list. A bot whose bot.toml names another benchmark says so under its figure.";

const BACKTEST_ABOUT =
  "The backtest behind the row. For a live run, the backtest it went live from, with its CAGR vs SPY; otherwise the screening backtest. Passes gate or fails n of 8 checks (click for which), and the three squares show whether it beat SPY in each third of the period.";

type SortKey = "bot" | "headline" | "backtest" | "cagr" | "drawdown" | "activity";

const negate = (value: number | null | undefined) => (value === null || value === undefined ? undefined : -value);

function own(entry: Entry): { cagr: number | undefined; drawdown: number | undefined } {
  const stats = entry.live?.run.live_stats;
  if (entry.live) {
    return stats && stats.sessions > 0 ? { cagr: stats.cagr, drawdown: -stats.max_drawdown } : { cagr: undefined, drawdown: undefined };
  }
  const glance = entry.bot.backtest;
  return { cagr: glance?.metrics.cagr ?? undefined, drawdown: negate(glance?.metrics.max_drawdown) };
}

function backtestOf(entry: Entry): Glance | null | undefined {
  return entry.live ? entry.live.source : entry.bot.backtest;
}

const SORT: SortColumns<Entry, SortKey> = {
  bot: { value: (entry) => entry.bot.name },
  headline: { value: headline, descending: true },
  backtest: { value: (entry) => (backtestOf(entry) ? -closeness(backtestOf(entry)) : undefined), descending: true },
  cagr: { value: (entry) => own(entry).cagr, descending: true },
  drawdown: { value: (entry) => own(entry).drawdown, descending: true },
  activity: { value: (entry) => (entry.bot.last_run_at ? Date.parse(entry.bot.last_run_at) : undefined), descending: true },
};

const finished = (status: Run["status"]) => status === "completed" || status === "failed" || status === "cancelled";

function patchRun(bots: Bot[], run: Run, update: (live: Live) => Live): Bot[] {
  return bots.map((bot) =>
    bot.name === run.bot && bot.live.some((live) => live.run.id === run.id) ? { ...bot, live: bot.live.map((live) => (live.run.id === run.id ? update(live) : live)) } : bot,
  );
}

function isGroup(value: string): value is Group {
  return GROUPS.some((group) => group === value);
}

function useBotRefresh(mutate: (update: (bots: Bot[]) => Bot[]) => void) {
  const inFlight = useRef(new Set<string>());
  const again = useRef(new Set<string>());
  const fetchBot = (name: string) => {
    if (inFlight.current.has(name)) {
      again.current.add(name);
      return;
    }
    inFlight.current.add(name);
    must(api.GET("/api/bots/{name}", { params: { path: { name } } }))
      .then(
        (detail) => mutate((current) => current.map((bot) => (bot.name === name ? detail.summary : bot))),
        (error: unknown) => notify(`${name}: ${errorMessage(error)}`),
      )
      .finally(() => {
        inFlight.current.delete(name);
        if (again.current.delete(name)) {
          fetchBot(name);
        }
      });
  };
  return useFrameBatch<string>((names) => new Set(names).forEach(fetchBot));
}

export function BotGrid() {
  const bots = useResource(() => must(api.GET("/api/bots")), []);
  const jobs = useResource(() => must(api.GET("/api/data/jobs")), []);
  const [marks, setMarks] = useState<Record<number, Mark>>({});
  const [filter, setFilter] = useQueryParam("q");
  const [groupParam, setGroup] = useQueryParam("stage");
  const [untestedOpen, setUntestedOpen] = useStored<boolean>("bots-untested-open", false, (value): value is boolean => typeof value === "boolean");
  const sorting = useSort("bot-list", SORT, { key: "headline", descending: true });
  const refreshBot = useBotRefresh(bots.mutate);

  const applyMarks = useFrameBatch<EquityEvent>((events) =>
    setMarks((current) => ({ ...current, ...Object.fromEntries(events.map((event) => [event.run_id, { at: event.at, equity: event.equity }])) })),
  );

  const liveRuns = (bots.data ?? []).flatMap((bot) => bot.live);
  const known = (id: number) => liveRuns.find((live) => live.run.id === id);

  const changed = (run: Run) => {
    if (finished(run.status) || !known(run.id)) {
      refreshBot(run.bot);
    } else {
      bots.mutate((current) => patchRun(current, run, (live) => ({ ...live, run: { ...live.run, status: run.status, desired: run.desired, message: run.message ?? null } })));
    }
  };

  useSpineEvents((event) => {
    if (event.type === "live_session") {
      setMarks((current) => Object.fromEntries(Object.entries(current).filter(([id]) => Number(id) !== event.run_id)));
      if (known(event.run_id)) {
        bots.mutate((current) => patchRun(current, event.summary.run, () => event.summary));
      } else {
        refreshBot(event.bot);
      }
    } else if (event.type === "equity" && known(event.run_id)) {
      applyMarks(event);
    } else if (event.type === "run_status" && event.mode === "live") {
      const live = known(event.run_id);
      if (!live || finished(event.status)) {
        refreshBot(event.bot);
      } else {
        const desired = event.status === "paused" ? live.run.desired : "running";
        bots.mutate((current) => patchRun(current, live.run, (entry) => ({ ...entry, run: { ...entry.run, status: event.status, desired } })));
      }
    } else if (event.type === "run_status") {
      if (finished(event.status)) {
        refreshBot(event.bot);
      } else {
        const now = new Date().toISOString();
        bots.mutate((current) =>
          current.map((bot) =>
            bot.name !== event.bot ? bot : { ...bot, last_run_at: event.status === "queued" ? now : (bot.last_run_at ?? null), status: bot.live.length > 0 ? bot.status : "backtesting" },
          ),
        );
      }
    } else if (event.type === "job") {
      jobs.mutate((current) => (current.some((job) => job.name === event.job.name) ? current.map((job) => (job.name === event.job.name ? event.job : job)) : [...current, event.job]));
    }
  });

  const all = entriesOf(bots.data ?? []);
  const group = isGroup(groupParam) ? groupParam : undefined;
  const found = all.filter((entry) => (!group || entry.group === group) && matches(filter, entry.bot.name, entry.bot.description));
  const searching = filter.trim() !== "" || group !== undefined;
  const groups = GROUPS.map((key) => ({ key, rows: sorting.order(found.filter((entry) => entry.group === key)) })).filter((section) => section.rows.length > 0);
  const visible = groups.flatMap((section) => (section.key === "untested" && !untestedOpen && !searching ? [] : section.rows));
  const chosen = useSelection(visible.map((entry) => entry.key));
  const scale = excessScale(all.map(headline));

  if (!bots.data) {
    return bots.error ? <Failure message={bots.error} onRetry={bots.reload} /> : <Loading shape="table" />;
  }
  if (bots.data.length === 0) {
    return <EmptyState title="No bots yet">Add a bot folder under bots/ and it shows up here, ready to backtest.</EmptyState>;
  }

  return (
    <div className="page-stack bots">
      <header className="page-head bots-head">
        <div>
          <h1>Bots</h1>
          <StateLine entries={all} />
        </div>
        <PageSearch label="Search bots" value={filter} onChange={setFilter} />
      </header>
      <StaleFailure resource={bots} />
      <Overview entries={all} jobs={jobs.data} marks={marks} />
      <section className="bots-list" aria-label="Bot list">
        <div className="list-bar">
          <nav className="segmented list-filter" aria-label="Show">
            <button type="button" aria-pressed={!group} className={group ? undefined : "active"} onClick={() => setGroup("")}>
              All <span className="num">{all.length}</span>
            </button>
            {GROUPS.map((key) => (
              <button key={key} type="button" aria-pressed={group === key} className={group === key ? "active" : undefined} onClick={() => setGroup(group === key ? "" : key)}>
                {GROUP_TEXT[key].name} <span className="num">{all.filter((entry) => entry.group === key).length}</span>
              </button>
            ))}
          </nav>
          <BenchmarkStrip entries={all} />
        </div>
        {groups.length === 0 ? (
          <div className="card">
            <EmptyState title={group ? `Nothing is ${GROUP_TEXT[group].name.toLowerCase()}` : "No bot matches"}>
              {group ? GROUP_TEXT[group].caption : `Nothing matches “${filter.trim()}”.`}
            </EmptyState>
          </div>
        ) : (
          <div className="card bot-table-wrap">
            <table className="table bot-list">
              <thead>
                <tr>
                  <SortHeader sorting={sorting} column="bot" className="name-col" select={<SelectAll selection={chosen} label="Select every bot shown" />}>
                    Bot
                  </SortHeader>
                  <SortHeader sorting={sorting} column="headline" numeric className="headline-col" info={<Info label="CAGR vs SPY">{HEADLINE_ABOUT}</Info>}>
                    CAGR vs SPY
                  </SortHeader>
                  <th scope="col" className="status-col">
                    Status
                  </th>
                  <SortHeader sorting={sorting} column="backtest" className="backtest-col" info={<Info label="Backtest">{BACKTEST_ABOUT}</Info>}>
                    Backtest
                  </SortHeader>
                  <SortHeader sorting={sorting} column="cagr" numeric className="own-col">
                    CAGR
                  </SortHeader>
                  <SortHeader sorting={sorting} column="drawdown" numeric className="own-col">
                    Max DD
                  </SortHeader>
                  <th scope="col" className="chart-col">
                    Equity vs SPY
                  </th>
                  <SortHeader sorting={sorting} column="activity" className="activity-col">
                    Last run
                  </SortHeader>
                  <th scope="col" className="next-col">
                    <span className="visually-hidden">Next step and actions</span>
                  </th>
                </tr>
              </thead>
              {groups.map((section) => {
                const collapsed = section.key === "untested" && !untestedOpen && !searching;
                return (
                  <tbody key={section.key} onKeyDown={moveBetweenRows}>
                    <tr className="group-row">
                      <th scope="colgroup" colSpan={9}>
                        <span className="group-head">
                          {section.key === "untested" && !searching ? (
                            <button type="button" className="group-toggle" aria-expanded={!collapsed} onClick={() => setUntestedOpen(!untestedOpen)}>
                              <Icon name="chevron" />
                              <span className="group-name">{GROUP_TEXT[section.key].name}</span>
                            </button>
                          ) : (
                            <span className="group-name">{GROUP_TEXT[section.key].name}</span>
                          )}
                          <span className="group-count num">{section.rows.length}</span>
                          <span className="group-caption">{GROUP_TEXT[section.key].caption}</span>
                          {section.key === "untested" && <UntestedActions entries={section.rows} />}
                        </span>
                      </th>
                    </tr>
                    {!collapsed &&
                      section.rows.map((entry) => <BotRow key={entry.key} entry={entry} mark={entry.live ? marks[entry.live.run.id] : undefined} chosen={chosen} scale={scale} onChanged={changed} />)}
                  </tbody>
                );
              })}
            </table>
          </div>
        )}
      </section>
      <div className="bulk-dock">
        <BulkBar selection={chosen} noun={chosen.keys.length === 1 ? "bot" : "bots"}>
          <Bulk entries={visible.filter((entry) => chosen.has(entry.key))} onDone={chosen.clear} />
        </BulkBar>
      </div>
    </div>
  );
}

function StateLine({ entries }: { entries: Entry[] }) {
  const live = entries.filter((entry) => entry.live).length;
  const running = entries.filter((entry) => entry.live && isRunning(entry.live.run)).length;
  const ready = entries.filter((entry) => entry.group === "ready").length;
  const backtesting = new Set(entries.filter((entry) => entry.bot.status === "backtesting").map((entry) => entry.bot.name)).size;
  const bots = new Set(entries.map((entry) => entry.bot.name)).size;
  const parts = [
    `${live} live paper ${live === 1 ? "run" : "runs"}${running < live ? ` (${live - running} paused)` : ""}`,
    `${ready} ready to go live`,
    ...(backtesting > 0 ? [`${backtesting} ${backtesting === 1 ? "bot" : "bots"} backtesting`] : []),
    `${bots} bots`,
  ];
  return (
    <p className="page-question state-line" role="status">
      {parts.join(" · ")}
    </p>
  );
}

type Window = { key: string; symbol: string; label: string; cagr: number | null | undefined; drawdown: number | null | undefined; bots: number };

function windows(entries: Entry[]): Window[] {
  const found = new Map<string, Window>();
  for (const entry of entries) {
    const glance = entry.live ? undefined : entry.bot.backtest;
    if (!glance) {
      continue;
    }
    const label = windowOf(glance);
    const key = `${glance.benchmark_symbol} ${label}`;
    const known = found.get(key);
    found.set(key, known ? { ...known, bots: known.bots + 1 } : { key, symbol: glance.benchmark_symbol, label, cagr: glance.benchmark.cagr, drawdown: negate(glance.benchmark.max_drawdown), bots: 1 });
  }
  return [...found.values()].toSorted((a, b) => b.bots - a.bots);
}

function BenchmarkStrip({ entries }: { entries: Entry[] }) {
  const shown = windows(entries);
  if (shown.length === 0) {
    return null;
  }
  return (
    <p className="benchmark-strip">
      <span className="benchmark-title">
        Benchmark
        <Info label="Benchmark">
          Every CAGR vs SPY figure subtracts the benchmark's own CAGR over exactly the same sessions, with dividends reinvested. The benchmark is the same for every bot over a
          window, so its figures are stated here once per window instead of on every row. A live run's benchmark is stated in the Live paper card.
        </Info>
      </span>
      {shown.map((window) => (
        <span key={window.key} className="benchmark-chip">
          <span className="benchmark-name">
            {window.symbol} · {window.label}
          </span>
          <span className="num">{fmt.change(window.cagr)}/yr</span>
          <span className="num muted">max DD {fmt.change(window.drawdown)}</span>
        </span>
      ))}
    </p>
  );
}

function UntestedActions({ entries }: { entries: Entry[] }) {
  const idle = entries.filter((entry) => entry.bot.status !== "backtesting").map((entry) => entry.bot.name);
  const [busy, setBusy] = useState(false);
  if (idle.length === 0) {
    return null;
  }
  const screen = async () => {
    setBusy(true);
    const queued = await settle(idle, String, (name) => must(api.POST("/api/bots/{name}/backtests", { params: { path: { name } }, body: { preset: "10y" } })));
    setBusy(false);
    if (queued.length > 0) {
      notify(`${counted(queued.length, "backtest")} queued on 10y`, "success");
    }
  };
  return (
    <button type="button" className="btn btn-xs group-action" disabled={busy} onClick={() => void screen()}>
      Backtest {idle.length} idle on 10y
    </button>
  );
}

function HeadlineCell({ entry, scale }: { entry: Entry; scale: number }) {
  const { live, bot } = entry;
  const stats = live?.run.live_stats;
  if (live) {
    if (!stats || stats.sessions === 0) {
      return (
        <>
          <span className="headline num muted">—</span>
          <span className="cell-sub">no close yet</span>
        </>
      );
    }
    return (
      <>
        <span className="headline-line">
          <span className={`headline num ${tone(stats.excess_cagr) ?? ""}`}>
            <Annualized value={stats.excess_cagr} sessions={stats.sessions} />
          </span>
          <ExcessBar value={stats.excess_cagr} scale={scale} />
        </span>
        <span className="cell-sub">
          live · {fmt.integer(stats.sessions)} {stats.sessions === 1 ? "session" : "sessions"}
        </span>
      </>
    );
  }
  const glance = bot.backtest;
  if (!glance) {
    return (
      <>
        <span className="headline num muted">—</span>
        <span className="cell-sub">no backtest of current code</span>
      </>
    );
  }
  const excess = excessCagr(glance);
  return (
    <>
      <span className="headline-line">
        <span className={`headline num ${tone(excess) ?? ""}`}>{fmt.change(excess)}</span>
        <ExcessBar value={excess} scale={scale} />
      </span>
      <span className="cell-sub">
        {windowOf(glance)}
        {glance.benchmark_symbol !== BENCHMARK && ` · vs ${glance.benchmark_symbol}`}
      </span>
    </>
  );
}

function BacktestCell({ entry }: { entry: Entry }) {
  if (entry.live) {
    return <SourceBacktest live={entry.live} />;
  }
  const glance = entry.bot.backtest;
  if (!glance) {
    return <span className="muted">—</span>;
  }
  return (
    <span className="source">
      <span className="source-checks">
        {glance.preset ? <GateResult checks={glance.failed_checks} run={glance.run_id} /> : <span className="cell-sub">custom period, not a screen</span>}
        <SubPeriods edges={glance.preset ? glance.sub_periods : []} count={false} />
      </span>
    </span>
  );
}

function StatusCell({ entry }: { entry: Entry }) {
  const { live, bot } = entry;
  if (live) {
    return (
      <span className="status-stack">
        <Verdict live={live} />
        {!isRunning(live.run) && <span className={`pill pill-${live.run.message ? "error" : "paused"}`}>{live.run.message ? "error" : "paused"}</span>}
      </span>
    );
  }
  return bot.status === "idle" ? null : <StatusPill status={bot.status} />;
}

function NextStep({ entry }: { entry: Entry }) {
  const { bot, live, group } = entry;
  if (live || bot.status === "backtesting") {
    return null;
  }
  if (group === "ready" && bot.backtest) {
    return <GoLiveButton bot={bot} glance={bot.backtest} primary={false} />;
  }
  return group === "failing" ? (
    <button type="button" className="btn btn-sm btn-ghost" onClick={() => launchSweep(bot.name)} aria-label={`Sweep ${bot.name}`}>
      Sweep…
    </button>
  ) : (
    <button type="button" className="btn btn-sm btn-ghost" onClick={() => launchBacktest(bot.name)} aria-label={`Backtest ${bot.name}`}>
      Backtest…
    </button>
  );
}

function BotRow({ entry, mark, chosen, scale, onChanged }: { entry: Entry; mark: Mark | undefined; chosen: ReturnType<typeof useSelection<string>>; scale: number; onChanged: (run: Run) => void }) {
  const { bot, live } = entry;
  const controls = useLiveControls(bot.name, live?.run, onChanged);
  const glance = bot.backtest;
  const variant = live ? variantOf(live.run.config.params as Record<string, unknown>, bot.params as Record<string, unknown>) : "";
  const target = live ? href({ page: "bot", name: bot.name, run: live.run.id }) : href({ page: "bot", name: bot.name });
  const actions: MenuAction[] = [
    ...(live ? [{ text: controls.toggleText, run: controls.toggle }] : []),
    { text: "Backtest…", run: () => launchBacktest(bot.name) },
    { text: "Sweep…", run: () => launchSweep(bot.name) },
    { text: "Research", run: () => navigate({ page: "research", name: bot.name }) },
    ...(!live && glance ? [{ text: `Go live with backtest #${glance.run_id}`, run: () => void goLive(bot.name, glance.run_id).then(onChanged, (error: unknown) => notify(`${bot.name}: ${errorMessage(error)}`)) }] : []),
    ...(!live ? [{ text: "Go live", run: controls.toggle }] : []),
    ...(controls.stop ? [{ text: "Stop…", run: controls.stop, danger: true }] : []),
  ];
  const figures = own(entry);
  const curve = live ? (live.run.live_stats?.curve ?? []) : (glance?.curve ?? []);

  return (
    <tr className={chosen.has(entry.key) ? "chosen" : undefined}>
      <th scope="row" className="name-col">
        <span className="name-cell">
          <SelectBox selection={chosen} value={entry.key} label={`Select ${bot.name}${live ? ` run #${live.run.id}` : ""}`} />
          <span className="bot-ident">
            <span className="bot-title">
              <a href={target}>{bot.name}</a>
              <span className="tf">{bot.timeframe}</span>
            </span>
            {variant ? <span className="variant-params mono">{variant}</span> : bot.description && <span className="bot-desc">{bot.description}</span>}
          </span>
        </span>
      </th>
      <td className="r headline-col">
        <HeadlineCell entry={entry} scale={scale} />
      </td>
      <td className="status-col">
        <StatusCell entry={entry} />
      </td>
      <td className="backtest-col">
        <BacktestCell entry={entry} />
      </td>
      <td className="r num own-col">{fmt.change(figures.cagr)}</td>
      <td className="r num own-col">{fmt.change(figures.drawdown)}</td>
      <td className="spark-cell chart-col">
        <Sparkline curve={curve} mark={mark} benchmark={`total-return ${live ? BENCHMARK : (glance?.benchmark_symbol ?? BENCHMARK)}`} />
      </td>
      <td className="muted nowrap activity-col">{bot.last_run_at ? <Ago at={bot.last_run_at} /> : "never"}</td>
      <td className="next-col">
        <span className="next-cell">
          <NextStep entry={entry} />
          <OverflowMenu label={`Actions for ${bot.name}${live ? ` run #${live.run.id}` : ""}`} actions={actions} />
          {controls.dialog}
        </span>
      </td>
    </tr>
  );
}

async function settle<S, T>(items: S[], name: (item: S) => string, start: (item: S) => Promise<T>): Promise<T[]> {
  const outcomes = await Promise.allSettled(items.map(start));
  outcomes.forEach((outcome, index) => {
    const item = items[index];
    if (outcome.status === "rejected" && item !== undefined) {
      notify(`${name(item)}: ${errorMessage(outcome.reason)}`);
    }
  });
  return outcomes.flatMap((outcome) => (outcome.status === "fulfilled" ? [outcome.value] : []));
}

function counted(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

function Bulk({ entries, onDone }: { entries: Entry[]; onDone: () => void }) {
  const [busy, setBusy] = useState(false);
  const liveRuns = entries.flatMap((entry) => (entry.live ? [entry.live.run] : []));
  const running = liveRuns.filter(isRunning);
  const paused = liveRuns.filter((run) => !isRunning(run));
  const idle = entries.filter((entry) => !entry.live).map((entry) => entry.bot.name);
  const runs = entries.flatMap((entry) => (entry.live ? [entry.live.run.id] : entry.bot.backtest ? [entry.bot.backtest.run_id] : []));

  const apply = async <T,>(items: T[], name: (item: T) => string, act: (item: T) => Promise<unknown>, done: string, undo?: (changed: Run[]) => ReturnType<typeof undoPause>) => {
    setBusy(true);
    const changed = await settle(items, name, act);
    setBusy(false);
    if (changed.length > 0) {
      notify(`${counted(changed.length, "run")} ${done}`, "success", undo?.(changed as Run[]));
      onDone();
    }
  };

  return (
    <>
      {running.length > 0 && (
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void apply(running, (run) => `${run.bot} #${run.id}`, (run) => pauseRun(run.id), "paused", undoPause)}>
          <Icon name="pause" />
          Pause {running.length}
        </button>
      )}
      {paused.length > 0 && (
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void apply(paused, (run) => `${run.bot} #${run.id}`, (run) => resumeRun(run.id), "live again")}>
          <Icon name="play" />
          Resume {paused.length}
        </button>
      )}
      {idle.length > 0 && (
        <span className="segmented segmented-sm" role="group" aria-label="Backtest the selected bots">
          <span className="segmented-label">Backtest</span>
          {SWEEP_PRESETS.map((preset: Preset) => (
            <button
              key={preset}
              type="button"
              disabled={busy}
              onClick={() => void apply(idle, String, (name) => must(api.POST("/api/bots/{name}/backtests", { params: { path: { name } }, body: { preset } })), `queued on ${preset}`)}
            >
              {preset}
            </button>
          ))}
        </span>
      )}
      {idle.length > 0 && (
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void apply(idle, String, (name) => goLive(name), "live")}>
          <Icon name="play" />
          Go live {idle.length}
        </button>
      )}
      <CompareButton runs={runs} />
    </>
  );
}
