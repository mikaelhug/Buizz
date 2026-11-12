import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { type SpineEvent, useSpineEvents } from "../api/stream";
import { fmt, paramsText, tone } from "../lib/format";
import { href, navigate, useQueryParam } from "../lib/route";
import { useFrameBatch } from "../lib/useFrameBatch";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { Icon } from "../ui/Icon";
import { Card, EmptyState, Stat } from "../ui/Kit";
import { Annualized, LIVE_COLUMNS, type Live } from "../ui/LiveTable";
import { type MenuAction, OverflowMenu } from "../ui/Menu";
import { Info } from "../ui/Popover";
import { BulkBar, CompareButton, SelectAll, SelectBox, useSelection } from "../ui/Selection";
import { type Mark, Sparkline } from "../ui/Sparkline";
import { SUB_PERIODS_ABOUT, SubPeriods } from "../ui/SubPeriods";
import { Ago, matches, PageSearch, SortHeader, type SortColumns, useSort } from "../ui/Table";
import { notify } from "../ui/Toaster";
import { goLive, isRunning, pauseRun, resumeRun, undoPause, useLiveControls } from "./BotActions";
import { launchBacktest, launchSweep } from "./Launch";
import { type Preset, SWEEP_PRESETS } from "./SweepDialog";
import "../styles/board.css";
import "../styles/bots.css";

type Bot = Schemas["BotSummary"];
type Run = Schemas["RunSummary"];
type Glance = Schemas["BacktestGlance"];
type EquityEvent = Extract<SpineEvent, { type: "equity" }>;

const STAGES = ["draft", "screened", "incubating", "tracking", "proven", "falsified"] as const;
type Stage = (typeof STAGES)[number];

const STAGE_TEXT: Record<Stage, { name: string; caption: string }> = {
  draft: { name: "Draft", caption: "no variant passes the gate" },
  screened: { name: "Screened", caption: "passed the backtest gate" },
  incubating: { name: "Incubating", caption: "live, under 20 sessions" },
  tracking: { name: "Tracking", caption: "live, inside its backtest band" },
  proven: { name: "Proven", caption: "live, beats SPY with confidence" },
  falsified: { name: "Falsified", caption: "live, left its backtest band" },
};

const STAGE_RANK: Record<Stage, number> = { proven: 0, tracking: 1, falsified: 2, incubating: 3, screened: 4, draft: 5 };

const QUESTION_ABOUT =
  "Every figure compares a bot with total-return SPY (dividends reinvested) over exactly the same sessions. A bot first screens in backtest: the screening backtest is the plateau variant on the longest preset period its current code ran with standard costs, risk and cash, else the default params, else the latest variant. Only live paper results can prove it.";

type Entry = { key: string; bot: Bot; live: Live | undefined; stage: Stage };

function stageOf(bot: Bot, live: Live | undefined): Stage {
  if (live) {
    const verdict = live.run.live_stats?.verdict ?? "incubating";
    return verdict === "no_backtest" ? "tracking" : verdict;
  }
  return bot.stage === "screened" ? "screened" : "draft";
}

function entriesOf(bots: Bot[]): Entry[] {
  return bots.flatMap((bot): Entry[] =>
    bot.live.length > 0
      ? bot.live.map((live) => ({ key: `run-${live.run.id}`, bot, live, stage: stageOf(bot, live) }))
      : [{ key: `bot-${bot.name}`, bot, live: undefined, stage: stageOf(bot, undefined) }],
  );
}

function excessCagr(glance: Glance | null | undefined): number | undefined {
  const own = glance?.metrics.cagr;
  const spy = glance?.benchmark.cagr;
  return own === null || own === undefined || spy === null || spy === undefined ? undefined : own - spy;
}

function glanceWhat(glance: Glance): string {
  return glance.preset ? `backtest ${glance.preset}` : `backtest ${glance.period.from.slice(0, 4)}–${glance.period.to.slice(0, 4)}`;
}

function headline(entry: Entry): number | undefined {
  return entry.live ? (entry.live.run.live_stats?.excess_cagr ?? undefined) : excessCagr(entry.bot.backtest);
}

type Maybe = number | null | undefined;
type Figures = { cagr?: Maybe; spyCagr?: Maybe; ret?: Maybe; spyRet?: Maybe; drawdown?: Maybe; spyDrawdown?: Maybe; thirds: number[] };

const negate = (value: number | null | undefined) => (value === null || value === undefined ? undefined : -value);

function figures(entry: Entry): Figures {
  const stats = entry.live?.run.live_stats;
  if (entry.live) {
    return stats && stats.sessions > 0
      ? { cagr: stats.cagr, spyCagr: stats.benchmark_cagr, ret: stats.total_return, spyRet: stats.benchmark_return, drawdown: negate(stats.max_drawdown), thirds: [] }
      : { thirds: [] };
  }
  const glance = entry.bot.backtest;
  return glance
    ? {
        cagr: glance.metrics.cagr,
        spyCagr: glance.benchmark.cagr,
        ret: glance.metrics.total_return,
        spyRet: glance.benchmark.total_return,
        drawdown: negate(glance.metrics.max_drawdown),
        spyDrawdown: negate(glance.benchmark.max_drawdown),
        thirds: glance.preset ? glance.sub_periods : [],
      }
    : { thirds: [] };
}

function variantOf(params: Record<string, unknown>, defaults: Record<string, unknown>): string {
  return paramsText(Object.fromEntries(Object.entries(params).filter(([name, value]) => JSON.stringify(value) !== JSON.stringify(defaults[name]))));
}

function needsAttention(entry: Entry): boolean {
  return entry.stage === "falsified" || (entry.live?.run.status === "paused" && Boolean(entry.live.run.message));
}

type SortKey = "bot" | "stage" | "headline" | "cagr" | "return" | "drawdown" | "thirds" | "activity";

const SORT: SortColumns<Entry, SortKey> = {
  bot: { value: (entry) => entry.bot.name },
  stage: { value: (entry) => (needsAttention(entry) ? -1 : STAGE_RANK[entry.stage]) },
  headline: { value: headline, descending: true },
  cagr: { value: (entry) => figures(entry).cagr, descending: true },
  return: { value: (entry) => figures(entry).ret, descending: true },
  drawdown: { value: (entry) => figures(entry).drawdown, descending: true },
  thirds: { value: (entry) => (figures(entry).thirds.length === 0 ? undefined : figures(entry).thirds.filter((edge) => edge > 0).length), descending: true },
  activity: { value: (entry) => (entry.bot.last_run_at ? Date.parse(entry.bot.last_run_at) : undefined), descending: true },
};

const finished = (status: Run["status"]) => status === "completed" || status === "failed" || status === "cancelled";

function patchRun(bots: Bot[], run: Run, update: (live: Live) => Live): Bot[] {
  return bots.map((bot) =>
    bot.name === run.bot && bot.live.some((live) => live.run.id === run.id) ? { ...bot, live: bot.live.map((live) => (live.run.id === run.id ? update(live) : live)) } : bot,
  );
}

function isStage(value: string): value is Stage {
  return STAGES.some((stage) => stage === value);
}

export function BotGrid() {
  const bots = useResource(() => must(api.GET("/api/bots")), []);
  const [marks, setMarks] = useState<Record<number, Mark>>({});
  const [filter, setFilter] = useQueryParam("q");
  const [stageParam, setStage] = useQueryParam("stage");
  const sorting = useSort("bots", SORT, { key: "stage", descending: false });

  const refreshBot = (name: string) =>
    must(api.GET("/api/bots/{name}", { params: { path: { name } } })).then(
      (detail) => bots.mutate((current) => current.map((bot) => (bot.name === name ? detail.summary : bot))),
      (error: unknown) => notify(`${name}: ${errorMessage(error)}`),
    );

  const applyMarks = useFrameBatch<EquityEvent>((events) =>
    setMarks((current) => ({ ...current, ...Object.fromEntries(events.map((event) => [event.run_id, { at: event.at, equity: event.equity }])) })),
  );

  const liveRuns = (bots.data ?? []).flatMap((bot) => bot.live);
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

  const all = entriesOf(bots.data ?? []);
  const stage = isStage(stageParam) ? stageParam : undefined;
  const shown = sorting.order(all.filter((entry) => (!stage || entry.stage === stage) && matches(filter, entry.bot.name, entry.bot.description)));
  const chosen = useSelection(shown.map((entry) => entry.key));

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
          <p className="page-question">
            Which bot beats total-return SPY, first in backtest and then live?
            <Info label="How bots are judged">{QUESTION_ABOUT}</Info>
          </p>
        </div>
        <PageSearch label="Search bots" value={filter} onChange={setFilter} />
      </header>
      <StaleFailure resource={bots} />
      <Pipeline entries={all} stage={stage} onStage={(next) => setStage(next === stage ? "" : (next ?? ""))} />
      <Highlights entries={all} />
      <section className="bots-list" aria-label="Bot list">
        {shown.length === 0 ? (
          <EmptyState title={stage ? `No bot is ${STAGE_TEXT[stage].name.toLowerCase()}` : "No bot matches"}>
            {stage ? STAGE_TEXT[stage].caption : `Nothing matches “${filter.trim()}”.`}
          </EmptyState>
        ) : (
          <div className="card table-wrap">
            <table className="table board bot-list">
              <thead>
                <tr>
                  <SortHeader sorting={sorting} column="bot" className="board-name" select={<SelectAll selection={chosen} label="Select every bot shown" />}>
                    Bot
                  </SortHeader>
                  <SortHeader sorting={sorting} column="stage" className="stage-col">
                    Stage
                  </SortHeader>
                  <SortHeader sorting={sorting} column="headline" numeric className="headline-cell" info={<Info label="CAGR vs SPY">{QUESTION_ABOUT}</Info>}>
                    CAGR vs SPY
                  </SortHeader>
                  <SortHeader sorting={sorting} column="cagr" numeric className="wide-col">
                    CAGR
                  </SortHeader>
                  <SortHeader sorting={sorting} column="return" numeric className="wide-col">
                    Return
                  </SortHeader>
                  <SortHeader sorting={sorting} column="drawdown" numeric className="wide-col">
                    Max DD
                  </SortHeader>
                  <SortHeader sorting={sorting} column="thirds" className="wider-col" info={<Info label="Thirds">{SUB_PERIODS_ABOUT}</Info>}>
                    Thirds
                  </SortHeader>
                  <th scope="col" className="chart-col">
                    vs total-return SPY
                  </th>
                  <SortHeader sorting={sorting} column="activity" className="activity-col">
                    Activity
                  </SortHeader>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {shown.map((entry) => (
                  <BotRow key={entry.key} entry={entry} mark={entry.live ? marks[entry.live.run.id] : undefined} chosen={chosen} onChanged={changed} />
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
      <div className="bulk-dock">
        <BulkBar selection={chosen} noun={chosen.keys.length === 1 ? "bot" : "bots"}>
          <Bulk entries={shown.filter((entry) => chosen.has(entry.key))} onDone={chosen.clear} />
        </BulkBar>
      </div>
    </div>
  );
}

function Pipeline({ entries, stage, onStage }: { entries: Entry[]; stage: Stage | undefined; onStage: (stage: Stage | undefined) => void }) {
  return (
    <nav className="pipeline" aria-label="Pipeline">
      {STAGES.map((step) => {
        const count = entries.filter((entry) => entry.stage === step).length;
        return (
          <button key={step} type="button" className={`pipeline-step step-${step}`} aria-pressed={stage === step} onClick={() => onStage(step)}>
            <span className={count > 0 ? "pipeline-count num" : "pipeline-count num zero"}>{count}</span>
            <span className="pipeline-name">{STAGE_TEXT[step].name}</span>
            <span className="pipeline-caption">{STAGE_TEXT[step].caption}</span>
          </button>
        );
      })}
    </nav>
  );
}

function best(entries: Entry[], score: (entry: Entry) => number | undefined): Entry | undefined {
  return entries
    .filter((entry) => score(entry) !== undefined)
    .toSorted((a, b) => (score(b) ?? 0) - (score(a) ?? 0))[0];
}

function Highlights({ entries }: { entries: Entry[] }) {
  const live = entries.filter((entry) => entry.live && entry.live.run.live_stats && entry.live.run.live_stats.sessions > 0);
  const judged = live.filter((entry) => entry.stage !== "incubating");
  const topLive = best(judged.length > 0 ? judged : live, headline);
  const screened = entries.filter((entry) => !entry.live && entry.stage === "screened");
  const backtested = entries.filter((entry) => !entry.live && entry.bot.backtest?.preset);
  const topScreened = best(screened.length > 0 ? screened : backtested, headline);
  const liveCount = entries.filter((entry) => entry.live).length;

  return (
    <div className="highlights">
      <Card title="Best live" label="Best live" className="highlight">
        {topLive?.live?.run.live_stats ? (
          <LiveHighlight entry={topLive} live={topLive.live} />
        ) : (
          <EmptyState
            title={liveCount > 0 ? `${liveCount} live ${liveCount === 1 ? "run is" : "runs are"} waiting for a session close` : "No bot is live yet"}
            action={
              liveCount > 0 || !topScreened ? undefined : screened.length > 0 && topScreened.bot.backtest ? (
                <GoLiveButton bot={topScreened.bot} glance={topScreened.bot.backtest} />
              ) : (
                <button type="button" className="btn btn-sm" onClick={() => launchSweep(topScreened.bot.name)}>
                  Sweep {topScreened.bot.name}…
                </button>
              )
            }
          >
            {liveCount > 0
              ? "Figures appear after each live run's first session close."
              : screened.length > 0
                ? `${screened.length} ${screened.length === 1 ? "bot passed" : "bots passed"} screening and can go live; only live paper results rank bots.`
                : topScreened
                  ? `No bot has passed screening yet. Sweep the closest, ${topScreened.bot.name}, for a variant that beats total-return SPY.`
                  : "Backtest a bot to start screening."}
          </EmptyState>
        )}
      </Card>
      <Card title={screened.length > 0 ? "Best screened" : "Closest to passing"} label="Best screened" className="highlight">
        {topScreened?.bot.backtest ? (
          <BacktestHighlight bot={topScreened.bot} glance={topScreened.bot.backtest} />
        ) : (
          <EmptyState title="Nothing backtested yet">Backtest or sweep a bot on a preset period to screen it against total-return SPY.</EmptyState>
        )}
      </Card>
    </div>
  );
}

function GoLiveButton({ bot, glance }: { bot: Bot; glance: Glance }) {
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
    <button type="button" className="btn btn-sm btn-primary" disabled={busy} onClick={start}>
      <Icon name="play" />
      Go live with {bot.name}
    </button>
  );
}

function LiveHighlight({ entry, live }: { entry: Entry; live: Live }) {
  const stats = live.run.live_stats;
  if (!stats) {
    return null;
  }
  return (
    <div className="highlight-body">
      <div className="highlight-who">
        <a className="highlight-name" href={href({ page: "bot", name: entry.bot.name, run: live.run.id })}>
          {entry.bot.name}
        </a>
        {LIVE_COLUMNS.verdict.cell(live)}
        <span className="muted">
          live {fmt.integer(stats.sessions)} {stats.sessions === 1 ? "session" : "sessions"}
        </span>
      </div>
      <div className="highlight-stats">
        <Stat size="lg" label="CAGR vs SPY" value={fmt.change(stats.excess_cagr)} tone={tone(stats.excess_cagr)} sub={`${fmt.change(stats.cagr)} vs ${fmt.change(stats.benchmark_cagr)}`} />
        <Stat label="Return" value={fmt.change(stats.total_return)} sub={`SPY ${fmt.change(stats.benchmark_return)}`} />
        <Stat label="Max drawdown" value={fmt.change(-stats.max_drawdown)} />
      </div>
      <Sparkline curve={stats.curve} mark={undefined} benchmark="total-return SPY" />
    </div>
  );
}

function BacktestHighlight({ bot, glance }: { bot: Bot; glance: Glance }) {
  const excess = excessCagr(glance);
  const variant = variantOf(glance.params, bot.params as Record<string, unknown>);
  return (
    <div className="highlight-body">
      <div className="highlight-who">
        <a className="highlight-name" href={href({ page: "bot", name: bot.name, run: glance.run_id })}>
          {bot.name}
        </a>
        <span className={`pill pill-${bot.stage}`}>{bot.stage}</span>
        <span className="muted">
          {glanceWhat(glance)}
          {variant && ` · ${variant}`}
        </span>
      </div>
      <div className="highlight-stats">
        <Stat size="lg" label="CAGR vs SPY" value={fmt.change(excess)} tone={tone(excess)} sub={`${fmt.change(glance.metrics.cagr)} vs ${fmt.change(glance.benchmark.cagr)}`} />
        <Stat label="Return" value={fmt.change(glance.metrics.total_return)} sub={`SPY ${fmt.change(glance.benchmark.total_return)}`} />
        <Stat
          label="Max drawdown"
          value={fmt.change(glance.metrics.max_drawdown === null || glance.metrics.max_drawdown === undefined ? undefined : -glance.metrics.max_drawdown)}
          sub={`SPY ${fmt.change(glance.benchmark.max_drawdown === null || glance.benchmark.max_drawdown === undefined ? undefined : -glance.benchmark.max_drawdown)}`}
        />
        {glance.sub_periods.length > 0 && (
          <Stat
            label={
              <>
                Thirds ahead
                <Info label="Thirds">{SUB_PERIODS_ABOUT}</Info>
              </>
            }
            value={`${glance.sub_periods.filter((edge) => edge > 0).length} of ${glance.sub_periods.length}`}
            sub={<SubPeriods edges={glance.sub_periods} count={false} />}
          />
        )}
      </div>
      <Sparkline curve={glance.curve} mark={undefined} benchmark="total-return SPY" />
    </div>
  );
}

function BotRow({ entry, mark, chosen, onChanged }: { entry: Entry; mark: Mark | undefined; chosen: ReturnType<typeof useSelection<string>>; onChanged: (run: Run) => void }) {
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
  const stats = live?.run.live_stats;
  const excess = headline(entry);

  return (
    <tr className={[chosen.has(entry.key) ? "chosen" : "", needsAttention(entry) ? "attention" : ""].join(" ").trim() || undefined}>
      <th scope="row" className="board-name">
        <SelectBox selection={chosen} value={entry.key} label={`Select ${bot.name}${live ? ` run #${live.run.id}` : ""}`} />
        <span className="bot-ident">
          <a href={target}>{bot.name}</a>
          <span className="tf">{bot.timeframe}</span>
          {variant ? <span className="variant-params mono">{variant}</span> : bot.description && <span className="bot-desc">{bot.description}</span>}
        </span>
      </th>
      <td className="stage-col">
        <span className="stage-cell">
          {live ? LIVE_COLUMNS.verdict.cell(live) : <span className={`pill pill-${entry.stage}`}>{entry.stage}</span>}
          {live && !isRunning(live.run) && <span className={`pill pill-${needsAttention(entry) ? "error" : "paused"}`}>{needsAttention(entry) ? "error" : "paused"}</span>}
        </span>
      </td>
      <td className="r headline-cell">
        {live ? (
          stats && stats.sessions > 0 ? (
            <>
              <span className={`headline num ${tone(excess) ?? ""}`}>
                <Annualized value={stats.excess_cagr} sessions={stats.sessions} />
              </span>
              <span className="cell-sub">live · {fmt.integer(stats.sessions)} sessions</span>
            </>
          ) : (
            <span className="cell-sub">live · first session pending</span>
          )
        ) : glance ? (
          <>
            <span className={`headline num ${tone(excess) ?? ""}`}>{fmt.change(excess)}</span>
            <span className="cell-sub" title={glance.preset ? undefined : `${glance.period.from} → ${glance.period.to}, not a preset period with standard costs, so it does not screen`}>
              {glanceWhat(glance)}
              {!glance.preset ? " · not screened" : glance.failed_checks.length === 0 ? " · passes gate" : ""}
            </span>
          </>
        ) : (
          <span className="cell-sub">no completed backtest</span>
        )}
      </td>
      <FigureCells figures={figures(entry)} />
      <td className="spark-cell chart-col">
        {live ? (
          <Sparkline curve={stats?.curve ?? []} mark={mark} benchmark="total-return SPY" />
        ) : glance ? (
          <Sparkline curve={glance.curve} mark={undefined} benchmark="total-return SPY" />
        ) : (
          <div className="sparkline sparkline-empty" aria-hidden="true" />
        )}
      </td>
      <td className="muted nowrap activity-col">{bot.last_run_at ? <Ago at={bot.last_run_at} /> : "—"}</td>
      <td className="actions-cell">
        <OverflowMenu label={`Actions for ${bot.name}${live ? ` run #${live.run.id}` : ""}`} actions={actions} />
        {controls.dialog}
      </td>
    </tr>
  );
}

function FigureCells({ figures: shown }: { figures: Figures }) {
  const pair = (own: number | null | undefined, spy: number | null | undefined) =>
    own === null || own === undefined ? (
      <span className="muted">—</span>
    ) : (
      <>
        <span className="num">{fmt.change(own)}</span>
        {spy !== null && spy !== undefined && <span className="cell-sub">SPY {fmt.change(spy)}</span>}
      </>
    );
  return (
    <>
      <td className="r nowrap wide-col">{pair(shown.cagr, shown.spyCagr)}</td>
      <td className="r nowrap wide-col">{pair(shown.ret, shown.spyRet)}</td>
      <td className="r nowrap wide-col">{pair(shown.drawdown, shown.spyDrawdown)}</td>
      <td className="wider-col"><SubPeriods edges={shown.thirds} /></td>
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
