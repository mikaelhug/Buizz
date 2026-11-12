import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { type SpineEvent, useSpineEvents } from "../api/stream";
import { fmt, label, paramsText, tone } from "../lib/format";
import { href, useQueryParam } from "../lib/route";
import { useFrameBatch } from "../lib/useFrameBatch";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { Icon } from "../ui/Icon";
import { LIVE_COLUMNS, type Live, type LiveKey, LiveHeader, liveSortColumns } from "../ui/LiveTable";
import { Info } from "../ui/Popover";
import { Ago, matches, PageSearch, SortHeader, type SortColumns, useSort } from "../ui/Table";
import { METRICS, MetricValue } from "../ui/Metrics";
import { BulkBar, CompareButton, SelectAll, SelectBox, useSelection } from "../ui/Selection";
import { type Mark, Sparkline } from "../ui/Sparkline";
import { notify } from "../ui/Toaster";
import { BotActions, goLive, isRunning, pauseRun, resumeRun, undoPause } from "./BotActions";
import { launchBacktest, launchSweep } from "./Launch";
import { type Preset, SWEEP_PRESETS } from "./SweepDialog";
import "../styles/board.css";
import "../styles/bots.css";

type Bot = Schemas["BotSummary"];
type Run = Schemas["RunSummary"];
type EquityEvent = Extract<SpineEvent, { type: "equity" }>;
type IdleKey = "name" | "stage" | "excess" | "return" | "drawdown" | "last_run_at";
type Glance = Schemas["BacktestGlance"];

const LIVE_SHOWN = ["since", "sessions", "excess_cagr", "cagr", "total_return", "benchmark_return", "excess_return", "max_drawdown", "verdict"] as const satisfies readonly LiveKey[];
type LiveOrder = "bot" | (typeof LIVE_SHOWN)[number] | "attention";
const STAGE_ORDER: Record<Bot["stage"], number> = { screened: 0, draft: 1, live: 2 };
const STAGE_ABOUT =
  "Draft: no variant of the bot's current code passes the screening gate yet. Screened: at least one variant passes it on the longest preset period it has run (completed with default costs, no data warnings or drawdown stop, at least 10 round trips, Sharpe above 0, not dominated by SPY). Live: a live paper run is going. The count says how many variants pass of those screened";

function liveParams(live: Live, defaults: Record<string, unknown> | undefined): string {
  const params = live.run.config.params as Record<string, unknown>;
  return paramsText(Object.fromEntries(Object.entries(params).filter(([name, value]) => JSON.stringify(value) !== JSON.stringify(defaults?.[name]))));
}

function attention(live: Live): number {
  if (live.run.live_stats?.verdict === "falsified" || (live.run.status === "paused" && live.run.message)) {
    return 0;
  }
  return isRunning(live.run) ? 2 : 1;
}

const LIVE_SORT: SortColumns<Live, LiveOrder> = {
  ...liveSortColumns(["bot", ...LIVE_SHOWN]),
  attention: { value: (live) => attention(live) * 3 + 1 - (live.run.live_stats?.probabilistic_sharpe ?? -0.5) },
};

function excessCagr(glance: Glance | null | undefined): number | undefined {
  const own = glance?.metrics.cagr;
  const spy = glance?.benchmark.cagr;
  return own === null || own === undefined || spy === null || spy === undefined ? undefined : own - spy;
}

const IDLE_SORT: SortColumns<Bot, IdleKey> = {
  name: { value: (bot) => bot.name },
  stage: { value: (bot) => STAGE_ORDER[bot.stage] },
  excess: { value: (bot) => excessCagr(bot.backtest), descending: true },
  return: { value: (bot) => bot.backtest?.metrics.total_return, descending: true },
  drawdown: { value: (bot) => bot.backtest?.metrics.max_drawdown },
  last_run_at: { value: (bot) => (bot.last_run_at ? Date.parse(bot.last_run_at) : undefined), descending: true },
};

const BACKTEST_ABOUT =
  "The screening backtest on the longest preset period the bot's current code has run with default costs, its own risk limits and the standard start cash: the plateau variant if one passes the gate, else the bot's default params, else its latest completed variant. Every SPY figure is total-return SPY (dividends reinvested) over the same sessions; CAGR vs SPY is the bot's CAGR minus SPY's. A figure that is undefined for the run, such as CAGR under a year, shows as —. A backtest screens a bot; only live paper results rank it.";

const finished = (status: Run["status"]) => status === "completed" || status === "failed" || status === "cancelled";

function patchRun(bots: Bot[], run: Run, update: (live: Live) => Live): Bot[] {
  return bots.map((bot) =>
    bot.name === run.bot && bot.live.some((live) => live.run.id === run.id) ? { ...bot, live: bot.live.map((live) => (live.run.id === run.id ? update(live) : live)) } : bot,
  );
}

export function BotGrid() {
  const bots = useResource(() => must(api.GET("/api/bots")), []);
  const [marks, setMarks] = useState<Record<number, Mark>>({});
  const [filter, setFilter] = useQueryParam("q");
  const liveSorting = useSort("bots-live", LIVE_SORT, { key: "attention", descending: false });
  const idleSorting = useSort("bots-idle", IDLE_SORT, { key: "stage", descending: false });

  const refreshBot = (name: string) =>
    must(api.GET("/api/bots/{name}", { params: { path: { name } } })).then(
      (detail) => bots.mutate((current) => current.map((bot) => (bot.name === name ? detail.summary : bot))),
      (error: unknown) => notify(`${name}: ${errorMessage(error)}`),
    );

  const applyMarks = useFrameBatch<EquityEvent>((events) =>
    setMarks((current) => ({ ...current, ...Object.fromEntries(events.map((event) => [event.run_id, { at: event.at, equity: event.equity }])) })),
  );

  const liveRuns = (bots.data ?? []).flatMap((bot) => bot.live);
  const defaults = new Map((bots.data ?? []).map((bot) => [bot.name, bot.params as Record<string, unknown>]));
  const known = (id: number) => liveRuns.find((live) => live.run.id === id);

  const changed = (run: Run) => {
    if (finished(run.status) || !known(run.id)) {
      void refreshBot(run.bot);
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
        void refreshBot(event.bot);
      }
    } else if (event.type === "equity" && known(event.run_id)) {
      applyMarks(event);
    } else if (event.type === "run_status" && event.mode === "live") {
      const live = known(event.run_id);
      if (!live || finished(event.status)) {
        void refreshBot(event.bot);
      } else {
        const desired = event.status === "paused" ? live.run.desired : "running";
        bots.mutate((current) => patchRun(current, live.run, (entry) => ({ ...entry, run: { ...entry.run, status: event.status, desired } })));
      }
    } else if (event.type === "run_status") {
      const now = new Date().toISOString();
      bots.mutate((current) => current.map((bot) => (bot.name === event.bot && event.status === "queued" ? { ...bot, last_run_at: now } : bot)));
      if (event.status === "completed" && bots.data?.some((bot) => bot.name === event.bot && bot.live.length === 0)) {
        void refreshBot(event.bot);
      }
    }
  });

  const live = liveSorting.order(liveRuns.filter((entry) => matches(filter, entry.run.bot)));
  const idle = idleSorting.order((bots.data ?? []).filter((bot) => bot.live.length === 0 && matches(filter, bot.name)));
  const liveChosen = useSelection(live.map((entry) => entry.run.id));
  const idleChosen = useSelection(idle.map((bot) => bot.name));

  if (!bots.data) {
    return bots.error ? <Failure message={bots.error} onRetry={bots.reload} /> : <Loading shape="table" />;
  }
  if (bots.data.length === 0) {
    return <p className="empty">No bots yet. Add a bot folder under bots/ and it shows up here.</p>;
  }
  const chosenRuns = live.filter((entry) => liveChosen.has(entry.run.id)).map((entry) => entry.run);

  return (
    <div className="bots">
      <header className="page-head">
        <h1>Bots</h1>
        <PageSearch label="Search bots" value={filter} onChange={setFilter} />
      </header>
      <StaleFailure resource={bots} />
      <section className="bots-section" aria-label="Live">
        <header className="section-head">
          <h2>
            Live <span className="count">{live.length}</span>
          </h2>
          {liveSorting.sort.key !== "attention" && (
            <button type="button" className="btn btn-xs btn-ghost" onClick={() => liveSorting.toggle("attention")}>
              Needs attention first
            </button>
          )}
        </header>
        {live.length === 0 ? (
          <p className="panel muted bots-empty">{filter.trim() ? "No live run matches." : "No live runs. Go live with a bot below; only live paper results rank bots."}</p>
        ) : (
          <div className="panel table-wrap board-wrap">
            <table className="table board">
              <thead>
                <tr>
                  <LiveHeader sorting={liveSorting} column="bot" className="board-name" select={<SelectAll selection={liveChosen} label="Select every live run" />} />
                  {LIVE_SHOWN.map((key) => (
                    <LiveHeader key={key} sorting={liveSorting} column={key} />
                  ))}
                  <th scope="col">
                    <span className="th-inner">
                      Since live vs SPY
                      <Info label="Since live vs SPY">Session-close equity since the run went live (solid) against the benchmark bought with the same cash (dashed)</Info>
                    </span>
                  </th>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {live.map((entry) => (
                  <tr key={entry.run.id} className={[attention(entry) === 0 ? "attention" : "", liveChosen.has(entry.run.id) ? "chosen" : ""].join(" ").trim() || undefined}>
                    <th scope="row" className="board-name">
                      <SelectBox selection={liveChosen} value={entry.run.id} label={`Select ${entry.run.bot} run #${entry.run.id}`} />
                      {LIVE_COLUMNS.bot.cell(entry)}
                      {liveParams(entry, defaults.get(entry.run.bot)) && <span className="variant-params mono">{liveParams(entry, defaults.get(entry.run.bot))}</span>}
                      {attention(entry) < 2 && (
                        <span className={`pill pill-${attention(entry) === 0 ? "error" : "paused"}`} title={entry.run.message ?? undefined}>
                          {attention(entry) === 0 ? "error" : "paused"}
                        </span>
                      )}
                    </th>
                    {LIVE_SHOWN.map((key) => (
                      <td key={key} className={LIVE_COLUMNS[key].numeric ? "r" : undefined}>
                        {LIVE_COLUMNS[key].cell(entry)}
                      </td>
                    ))}
                    <td className="spark-cell">
                      <Sparkline curve={entry.run.live_stats?.curve ?? []} mark={marks[entry.run.id]} benchmark={entry.run.config.benchmark} />
                    </td>
                    <td className="actions-cell">
                      <BotActions name={entry.run.bot} live={entry.run} onChanged={changed} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
      <section className="bots-section" aria-label="Not live">
        <header className="section-head">
          <h2>
            Not live <span className="count">{idle.length}</span>
          </h2>
        </header>
        {idle.length > 0 && (
          <div className="panel table-wrap">
            <table className="table board idle-board">
              <thead>
                <tr>
                  <SortHeader sorting={idleSorting} column="name" className="board-name" rowSpan={2} select={<SelectAll selection={idleChosen} label="Select every bot not live" />}>
                    Bot
                  </SortHeader>
                  <SortHeader sorting={idleSorting} column="stage" rowSpan={2} info={<Info label="Stage">{STAGE_ABOUT}</Info>}>
                    Stage
                  </SortHeader>
                  <th scope="colgroup" colSpan={5} className="group-head">
                    <span className="th-inner">
                      Screening backtest vs total-return SPY
                      <Info label="Screening backtest">{BACKTEST_ABOUT}</Info>
                    </span>
                  </th>
                  <SortHeader sorting={idleSorting} column="last_run_at" rowSpan={2}>
                    Last activity
                  </SortHeader>
                  <th scope="col" rowSpan={2}>
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
                <tr>
                  <th scope="col">Period</th>
                  <SortHeader sorting={idleSorting} column="excess" numeric>
                    CAGR vs SPY
                  </SortHeader>
                  <SortHeader sorting={idleSorting} column="return" numeric>
                    Return
                  </SortHeader>
                  <SortHeader sorting={idleSorting} column="drawdown" numeric>
                    Max DD
                  </SortHeader>
                  <th scope="col">vs SPY</th>
                </tr>
              </thead>
              <tbody>
                {idle.map((bot) => (
                  <tr key={bot.name} className={idleChosen.has(bot.name) ? "chosen" : undefined}>
                    <th scope="row" className="board-name">
                      <SelectBox selection={idleChosen} value={bot.name} label={`Select ${bot.name}`} />
                      <a href={href({ page: "bot", name: bot.name })}>{bot.name}</a>
                      <span className="tf">{bot.timeframe}</span>
                    </th>
                    <td>
                      <span className={`stage stage-${bot.stage}`}>{label(bot.stage)}</span>
                      {bot.screening && (
                        <a className="screening muted" href={href({ page: "research", name: bot.name, preset: bot.screening.preset })}>
                          {bot.screening.passing} of {bot.screening.variants} pass
                        </a>
                      )}
                    </td>
                    <BacktestCells bot={bot} />
                    <td className="num muted nowrap">{bot.last_run_at ? <Ago at={bot.last_run_at} /> : "—"}</td>
                    <td className="actions-cell">
                      <span className="bot-actions">
                        <button type="button" className="btn btn-sm" onClick={() => launchBacktest(bot.name)}>
                          <Icon name="play" />
                          Backtest
                        </button>
                        <button type="button" className="btn btn-sm btn-ghost" onClick={() => launchSweep(bot.name)}>
                          Sweep
                        </button>
                        <BotActions name={bot.name} live={undefined} onChanged={changed} />
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
      <div className="bulk-dock">
        <BulkBar selection={liveChosen} noun={chosenRuns.length === 1 ? "live run" : "live runs"}>
          <LiveBulk runs={chosenRuns} onDone={liveChosen.clear} />
        </BulkBar>
        <BulkBar selection={idleChosen} noun={idleChosen.keys.length === 1 ? "bot" : "bots"}>
          <IdleBulk names={idleChosen.keys} onDone={idleChosen.clear} />
        </BulkBar>
      </div>
    </div>
  );
}

function BacktestCells({ bot }: { bot: Bot }) {
  const glance = bot.backtest;
  if (!glance) {
    return (
      <td colSpan={5} className="muted not-backtested">
        Not backtested on its current code
      </td>
    );
  }
  const excess = excessCagr(glance);
  const variant = paramsText(Object.fromEntries(Object.entries(glance.params).filter(([key, value]) => JSON.stringify(value) !== JSON.stringify((bot.params as Record<string, unknown>)[key]))));
  const target = href({ page: "bot", name: bot.name, run: glance.run_id });
  return (
    <>
      <td className="nowrap">
        <a className="glance-period" href={target}>
          {glance.preset}
        </a>
        {glance.plateau && <span className="pill pill-plateau">plateau</span>}
        {variant && <span className="variant-params mono">{variant}</span>}
      </td>
      <td className="r">
        <span className={`num ${tone(excess) ?? ""}`}>{fmt.change(excess)}</span>
        <span className="cell-sub num">CAGR {fmt.change(glance.metrics.cagr)}</span>
      </td>
      <td className="r">
        <span className={`num ${tone(glance.metrics.total_return) ?? ""}`}>{fmt.change(glance.metrics.total_return)}</span>
        <span className="cell-sub num">SPY {fmt.change(glance.benchmark.total_return)}</span>
      </td>
      <td className="r">
        <MetricValue metrics={glance.metrics} metric="max_drawdown" />
        <span className="cell-sub num">SPY {METRICS.max_drawdown.render(glance.benchmark.max_drawdown)}</span>
      </td>
      <td className="spark-cell">
        <a href={target} className="spark-link" aria-label={`Backtest run #${glance.run_id} of ${bot.name}`}>
          <Sparkline curve={glance.curve} mark={undefined} benchmark="total-return SPY" />
        </a>
      </td>
    </>
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

function LiveBulk({ runs, onDone }: { runs: Run[]; onDone: () => void }) {
  const [busy, setBusy] = useState(false);
  const running = runs.filter(isRunning);
  const paused = runs.filter((run) => !isRunning(run));

  const apply = async (chosen: Run[], act: (id: number) => Promise<Run>, done: string) => {
    setBusy(true);
    const changed = await settle(chosen, (run) => `${run.bot} #${run.id}`, (run) => act(run.id));
    setBusy(false);
    if (changed.length > 0) {
      notify(`${counted(changed.length, "run")} ${done}`, "success", done === "paused" ? undoPause(changed) : undefined);
      onDone();
    }
  };

  return (
    <>
      {running.length > 0 && (
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void apply(running, pauseRun, "paused")}>
          <Icon name="pause" />
          Pause {running.length}
        </button>
      )}
      {paused.length > 0 && (
        <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void apply(paused, resumeRun, "live again")}>
          <Icon name="play" />
          Resume {paused.length}
        </button>
      )}
      <CompareButton runs={runs.map((run) => run.id)} />
    </>
  );
}

function IdleBulk({ names, onDone }: { names: string[]; onDone: () => void }) {
  const [busy, setBusy] = useState(false);

  const backtest = async (preset: Preset) => {
    setBusy(true);
    const started = await settle(names, String, (name) => must(api.POST("/api/bots/{name}/backtests", { params: { path: { name } }, body: { preset } })));
    setBusy(false);
    if (started.length > 0) {
      notify(`${counted(started.length, "backtest")} queued on ${preset}`, "success");
      onDone();
    }
  };

  const live = async () => {
    setBusy(true);
    const started = await settle(names, String, (name) => goLive(name));
    setBusy(false);
    if (started.length > 0) {
      notify(`${counted(started.length, "bot")} live`, "success");
      onDone();
    }
  };

  return (
    <>
      <span className="segmented segmented-sm" role="group" aria-label="Backtest the selected bots">
        <span className="segmented-label">Backtest</span>
        {SWEEP_PRESETS.map((preset) => (
          <button key={preset} type="button" disabled={busy} onClick={() => void backtest(preset)}>
            {preset}
          </button>
        ))}
      </span>
      <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void live()}>
        <Icon name="play" />
        Go live
      </button>
    </>
  );
}
