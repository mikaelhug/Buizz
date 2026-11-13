import { type KeyboardEvent, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { api, errorMessage, must, mustSucceed, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { ABSENT, fmt, label, tone } from "../lib/format";
import { BOT_TABS, type BotTab, href, navigate, type Route, useQueryParam } from "../lib/route";
import { appendPoints } from "../lib/series";
import { useFrameBatch } from "../lib/useFrameBatch";
import { PAGE_SIZE, usePaged } from "../lib/usePaged";
import { useResource } from "../lib/useResource";
import { chartSync } from "../ui/chart/sync";
import { EquityChart } from "../ui/EquityChart";
import { Failure, LoadMore, Loading, StaleFailure } from "../ui/Feedback";
import { METRICS, MetricGroups, MetricValue } from "../ui/Metrics";
import { monthlyCalendar, MonthlyReturns } from "../ui/MonthlyReturns";
import { Info, Popover } from "../ui/Popover";
import { firstClose, JUDGED_FROM_SESSION, type Live, verdictLabel, YEAR_SESSIONS } from "../ui/LiveTable";
import { sourceSentence } from "../ui/Source";
import { Sparkline } from "../ui/Sparkline";
import { RunBadges, StatusPill } from "../ui/Status";
import { SymbolChart, SymbolLink } from "../ui/SymbolChart";
import { matches, moveBetweenRows, openRow, SortHeader, type SortColumns, TableFilter, useFlash, useSort } from "../ui/Table";
import { Icon } from "../ui/Icon";
import { notify } from "../ui/Toaster";
import { CHECKS } from "../ui/Gate";
import { goLive } from "./BotActions";
import { Trades } from "./Trades";

type Run = Schemas["RunSummary"];
type SparkPoint = Schemas["SparkPoint"];
type Curve = Omit<Schemas["EquitySeries"], "points"> & { points: SparkPoint[] };
type Row = [string, ReactNode];

const DAILY_BARS_JOB = "bars_1d";
const PICKER_RUNS = 12;
const MARK_KINDS = ["risk_breach", "status"] as const;

const TAB_LABELS: Record<BotTab, string> = {
  overview: "Overview",
  trades: "Trades",
  orders: "Orders",
  events: "Positions & events",
  logs: "Logs",
  runs: "History",
};

type Props = {
  name: string;
  runId: number;
  pinned: boolean;
  tab: BotTab;
  symbol: string | undefined;
  botHash: string;
  behaviourHash: string;
  recent: Run[];
  liveParams: ReadonlySet<string>;
  gate: Schemas["GateCheck"][] | undefined;
  live: Live | undefined;
  runs: ReactNode;
};

export function RunView({ name, runId, pinned, tab, symbol, botHash, behaviourHash, recent, liveParams, gate, live, runs }: Props) {
  const run = useResource(() => must(api.GET("/api/runs/{id}", { params: { path: { id: runId } } })), [runId]);
  const tabRoute = (target: BotTab): Route => ({ page: "bot", name, ...(pinned ? { run: runId } : {}), tab: target });
  const tabHref = (target: BotTab) => href(tabRoute(target));
  const symbolHref = (shown: string) => href({ page: "bot", name, ...(pinned ? { run: runId } : {}), symbol: shown });

  const mark = useFrameBatch<SparkPoint>((points) => {
    const last = points.at(-1);
    if (last) {
      run.mutate((current) => ({ ...current, equity: last.equity, clock: last.at }));
    }
  });

  useSpineEvents((event) => {
    if (!("run_id" in event) || event.run_id !== runId) {
      return;
    }
    if (event.type === "run_status") {
      run.reload();
    } else if (event.type === "equity") {
      mark({ at: event.at, equity: event.equity });
    } else if (event.type === "usage") {
      run.mutate((current) => ({ ...current, usage: event.usage }));
    } else if (event.type === "live_session") {
      run.mutate((current) => ({ ...current, live_stats: event.summary.run.live_stats ?? null }));
    }
  });

  if (!run.data) {
    return run.error ? <Failure message={run.error} onRetry={run.reload} /> : <Loading shape="chart" />;
  }
  return (
    <>
      <StaleFailure resource={run} />
      <RunBar run={run.data} name={name} tab={tab} botHash={botHash} behaviourHash={behaviourHash} recent={recent} liveParams={liveParams} gate={gate} logsHref={tabHref("logs")} runsHref={tabHref("runs")} />
      <Tabs tab={tab} routeFor={tabRoute} />
      <div className="tab-panel" role="tabpanel" id="run-tabpanel" aria-labelledby={`tab-${tab}`}>
        {tab === "overview" && <Overview run={run.data} live={live} symbol={symbol} gate={gate} closeHref={tabHref("overview")} />}
        {tab === "trades" && <Trades runId={runId} positionsHref={tabHref("events")} symbolHref={symbolHref} />}
        {tab === "orders" && <Orders runId={runId} active={!run.data.finished_at} symbolHref={symbolHref} />}
        {tab === "events" && (
          <div className="split">
            <Positions runId={runId} clock={run.data.clock} symbolHref={symbolHref} />
            <Events runId={runId} />
          </div>
        )}
        {tab === "logs" && <BotOutput runId={runId} />}
        {tab === "runs" && runs}
      </div>
    </>
  );
}

function Tabs({ tab, routeFor }: { tab: BotTab; routeFor: (tab: BotTab) => Route }) {
  const list = useRef<HTMLDivElement>(null);
  const onKeyDown = (event: KeyboardEvent) => {
    const index = BOT_TABS.indexOf(tab);
    const moves: Record<string, number> = { ArrowRight: index + 1, ArrowLeft: index - 1, Home: 0, End: BOT_TABS.length - 1 };
    const target = moves[event.key];
    if (target === undefined) {
      return;
    }
    event.preventDefault();
    const next = BOT_TABS[(target + BOT_TABS.length) % BOT_TABS.length] ?? "overview";
    navigate(routeFor(next));
    list.current?.querySelector<HTMLElement>(`#tab-${next}`)?.focus();
  };

  useEffect(() => {
    const strip = list.current;
    const active = strip?.querySelector<HTMLElement>(`#tab-${tab}`);
    if (strip && active) {
      strip.scrollTo({ left: active.offsetLeft - (strip.clientWidth - active.offsetWidth) / 2 });
    }
  }, [tab]);

  return (
    <div ref={list} className="tabs" role="tablist" aria-label="Run views" onKeyDown={onKeyDown}>
      {BOT_TABS.map((option) => (
        <a
          key={option}
          id={`tab-${option}`}
          role="tab"
          href={href(routeFor(option))}
          aria-selected={option === tab}
          aria-controls="run-tabpanel"
          tabIndex={option === tab ? 0 : -1}
          className={option === tab ? "tab active" : "tab"}
        >
          {TAB_LABELS[option]}
        </a>
      ))}
    </div>
  );
}

function awaitingData(run: Run): boolean {
  return run.mode === "live" && !run.clock && (run.status === "starting" || run.status === "running");
}

function period(run: Run): string {
  return `${run.config.start} → ${run.config.end ?? "now"}`;
}

function universe(spec: Schemas["UniverseSpec"]): string {
  if ("symbols" in spec) return spec.symbols.join(" ");
  if ("index" in spec) return `${INDEX_NAMES[spec.index]} members as of each session`;
  const kind = spec.asset_class ? ` ${ASSET_CLASS_NAMES[spec.asset_class]}` : "";
  return `Top ${spec.top_dollar_volume}${kind} by dollar volume`;
}

const INDEX_NAMES: Record<Schemas["StockIndex"], string> = { sp500: "S&P 500" };
const ASSET_CLASS_NAMES: Record<Schemas["AssetClass"], string> = { stock: "stocks", etf: "ETFs" };

function dollars(value: string): string {
  return `$${fmt.price(value)}`;
}

function spreadText(cost: Schemas["CostModel"]): string {
  const scale = Number(cost.spread_scale ?? 1);
  if (!scale) return `${cost.slippage_bps} bps of price`;
  const estimate = scale === 1 ? "the estimated half-spread" : `${cost.spread_scale} × the estimated half-spread`;
  return `${cost.slippage_bps} bps of price, or ${estimate} if wider`;
}

function costSummary(cost: Schemas["CostModel"]): string {
  const floor = Number(cost.spread_scale ?? 1) ? `≥ ${cost.slippage_bps}` : cost.slippage_bps;
  const impact = Number(cost.impact_coefficient ?? 1) ? " + impact" : "";
  return `${floor} bps${impact} · ${dollars(cost.commission_per_share)}/sh`;
}

function costRows(cost: Schemas["CostModel"]): Row[] {
  const perShare = Number(cost.slippage_per_share ?? 0);
  const impact = Number(cost.impact_coefficient ?? 1);
  return [
    ["Slippage", perShare ? `${spreadText(cost)} + ${dollars(cost.slippage_per_share)} a share` : spreadText(cost)],
    ["Impact", impact ? `${cost.impact_coefficient} × daily volatility × √(shares / average volume)` : "off"],
    ["Commission", `${dollars(cost.commission_per_share)} a share, min ${dollars(cost.commission_min)}`],
    ...(cost.commission_tiers ?? []).map((tier): Row => [`Above ${fmt.integer(tier.above)} sh/mo`, `${dollars(tier.per_share)} a share`]),
    ["Volume cap", `${fmt.percent(cost.adv_participation)} of ${cost.adv_sessions}-session average volume`],
    ["Borrow", `${fmt.percent(cost.borrow_rate)} a year on shorts`],
    ["Financing", `T-bill + ${fmt.percent(cost.financing_spread)} on negative cash`],
  ];
}

function limit(value: string | null | undefined): string {
  return value ? fmt.percent(value) : "none";
}

function riskRows(risk: Schemas["RiskLimits"]): Row[] {
  return [
    ["Max position", limit(risk.max_position_weight)],
    ["Max gross", limit(risk.max_gross_exposure)],
    ["Drawdown stop", limit(risk.max_drawdown)],
  ];
}

function paramValue(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

function reproRows(run: Run, botHash: string, behaviourHash: string): Row[] {
  const repro = run.reproducibility;
  if (!repro) {
    return [];
  }
  const ownCode: Row[] = repro.behaviour_hash
    ? [["Own code", <Hash value={repro.behaviour_hash} note={repro.behaviour_hash === behaviourHash ? undefined : "changed since"} />]]
    : [];
  const sameCode = repro.behaviour_hash ? repro.behaviour_hash === behaviourHash : repro.bot_hash === botHash;
  const codeNote = repro.bot_hash === botHash ? undefined : sameCode ? "shared code changed since" : "changed since";
  return [
    ["Bot code", <Hash value={repro.bot_hash} note={codeNote} />],
    ...ownCode,
    ["Engine", repro.engine_version],
    ["Seed", String(repro.seed)],
    ["Data through", fmt.market(repro.data_watermark)],
    ["Facts read", repro.recipe_versions.length > 0 ? repro.recipe_versions.map((read) => `${read.recipe} version ${read.version}`).join(", ") : "none"],
    ["Result", run.result_hash ? <Hash value={run.result_hash} /> : "once the run finishes"],
  ];
}

function Hash({ value, note }: { value: string; note?: string | undefined }) {
  return (
    <>
      <span className="mono">{value}</span>
      {note && <span className="hash-note">{note}</span>}
    </>
  );
}

function KeyValues({ rows, mono = false }: { rows: Row[]; mono?: boolean }) {
  return (
    <dl className="key-values">
      {rows.map(([key, value]) => (
        <div key={key}>
          <dt className={mono ? "mono" : undefined}>{key}</dt>
          <dd>{value}</dd>
        </div>
      ))}
    </dl>
  );
}

type RunBarProps = { run: Run; name: string; tab: BotTab; botHash: string; behaviourHash: string; recent: Run[]; liveParams: ReadonlySet<string>; gate: Schemas["GateCheck"][] | undefined; logsHref: string; runsHref: string };

function RunBar({ run, name, tab, botHash, behaviourHash, recent, liveParams, gate, logsHref, runsHref }: RunBarProps) {
  const repro = reproRows(run, botHash, behaviourHash);
  const [starting, setStarting] = useState(false);
  const promotable = run.mode === "backtest" && run.status === "completed" && gate?.length === 0 && !liveParams.has(JSON.stringify(run.config.params));
  const promote = () => {
    setStarting(true);
    goLive(name, run.id).then(
      (live) => {
        notify(`${name} live with run #${run.id}'s params · run #${live.id}`, "success");
        navigate({ page: "bot", name, run: live.id });
      },
      (error: unknown) => {
        notify(`${name}: ${errorMessage(error)}`);
        setStarting(false);
      },
    );
  };
  return (
    <section className="run-bar" aria-label="Run">
      <div className="run-bar-head">
        <RunPicker run={run} name={name} tab={tab} recent={recent} runsHref={runsHref} />
        <span className={`mode-badge mode-${run.mode}`}>{run.mode === "live" ? "Live paper" : "Backtest"}</span>
        <StatusPill status={run.status} />
        <RunBadges run={run} />
        <Info label="Reproducibility">
          {repro.length > 0 ? (
            <>
              <p className="popover-text">The same bot code, engine, seed, data and recipe versions give the same result hash.</p>
              <KeyValues rows={repro} />
            </>
          ) : (
            "Recorded once the run starts: bot code hash, engine version, seed and the data it saw."
          )}
        </Info>
        {promotable && (
          <button type="button" className="btn btn-sm run-promote" disabled={starting} onClick={promote}>
            <Icon name="play" />
            Go live with these params
          </button>
        )}
      </div>
      {run.message && (
        <p className="run-message" role="status">
          {run.message}
          {run.status === "failed" && tab !== "logs" && (
            <>
              {" "}
              <a href={logsHref}>See the bot's output</a>
            </>
          )}
        </p>
      )}
      <dl className="facts">
        <div>
          <dt>Period</dt>
          <dd>{period(run)}</dd>
        </div>
        {run.mode === "live" && run.source_run !== null && run.source_run !== undefined && (
          <div>
            <dt>From backtest</dt>
            <dd>
              <a href={href({ page: "bot", name, run: run.source_run })}>#{run.source_run}</a>
            </dd>
          </div>
        )}
        <div>
          <dt>Clock</dt>
          <dd>{awaitingData(run) ? <span className="muted">waiting for market data since {fmt.market(run.started_at ?? run.created_at)}</span> : fmt.market(run.clock)}</dd>
        </div>
        <div>
          <dt>Equity</dt>
          <dd className="num">
            {fmt.money(run.equity)}
            <span className="facts-sub">
              {" of "}
              {fmt.money(run.config.start_cash)}
            </span>
          </dd>
        </div>
        <div className="facts-wide">
          <dt>Universe</dt>
          <dd className="ellipsis">{universe(run.config.universe)}</dd>
        </div>
        <div>
          <dt>Costs</dt>
          <dd>
            {costSummary(run.config.cost)}
            <Info label="Cost model">
              <KeyValues rows={costRows(run.config.cost)} />
            </Info>
          </dd>
        </div>
        {run.usage.processes > 0 && (
          <div>
            <dt>
              Bot peak
              <Info label="Bot processes">
                The most memory any of this run's bot processes held, Python included, and how long they ran in all. A live run starts one process per session.
              </Info>
            </dt>
            <dd className="num">
              {fmt.memory(run.usage.peak_memory)}
              <span className="facts-sub">
                {" · "}
                {fmt.integer(run.usage.processes)} {run.usage.processes === 1 ? "process" : "processes"}, {fmt.duration(run.usage.seconds)}
              </span>
            </dd>
          </div>
        )}
      </dl>

    </section>
  );
}

function RunPicker({ run, name, tab, recent, runsHref }: { run: Run; name: string; tab: BotTab; recent: Run[]; runsHref: string }) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  const shown = recent.slice(0, PICKER_RUNS);
  return (
    <h2 className="run-title">
      <button
        type="button"
        className="run-picker"
        aria-haspopup="dialog"
        aria-expanded={anchor !== null}
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
      >
        Run #{run.id}
      </button>
      {anchor && (
        <Popover anchor={anchor} label="Switch run" width={460} onClose={() => setAnchor(null)}>
          <div className="popover-title">Recent runs</div>
          <ul className="run-options">
            {shown.map((option) => (
              <li key={option.id}>
                <a
                  href={href({ page: "bot", name, run: option.id, tab })}
                  className="run-option"
                  aria-current={option.id === run.id ? "true" : undefined}
                  onClick={() => setAnchor(null)}
                >
                  <span className="run-option-id">#{option.id}</span>
                  <span className="run-option-what">
                    {option.mode} · {period(option)}
                  </span>
                  <StatusPill status={option.status} />
                  <MetricValue metrics={option.metrics} metric="total_return" />
                </a>
              </li>
            ))}
          </ul>
          <a className="run-options-all" href={runsHref} onClick={() => setAnchor(null)}>
            All runs
          </a>
        </Popover>
      )}
    </h2>
  );
}

async function markEvents(runId: number): Promise<Schemas["RunEvent"][]> {
  const events: Schemas["RunEvent"][] = [];
  let after: number | undefined;
  do {
    const page = await must(
      api.GET("/api/runs/{id}/events", { params: { path: { id: runId }, query: { kind: [...MARK_KINDS], limit: PAGE_SIZE, ...(after === undefined ? {} : { after }) } } }),
    );
    events.push(...page.items);
    after = page.next_after ?? undefined;
  } while (after !== undefined);
  return events;
}

function isMark(event: Schemas["RunEvent"]): boolean {
  return MARK_KINDS.some((kind) => kind === event.kind.type);
}

function signedYearly(value: number): string {
  return `${fmt.change(Math.abs(value))}/yr`.replace(/^[+-]/, "");
}

type HeadlineProps = {
  run: Run;
  live: Live | undefined;
  metrics: Schemas["Metrics"] | null | undefined;
  benchmark: Schemas["Metrics"] | undefined;
  pending: boolean;
  gate: Schemas["GateCheck"][] | undefined;
};

function Headline({ run, live, metrics, benchmark, pending, gate }: HeadlineProps) {
  const stats = run.live_stats;
  if (run.mode === "live") {
    const current = live && { ...live, run };
    if (!stats || stats.sessions === 0) {
      const starts = current && firstClose(current, true);
      return (
        <p className="run-headline">
          Live paper since {run.config.start}.{" "}
          <strong>{starts ? `It trades for the first time at the close of ${starts}` : "It trades for the first time at the next session close"}</strong>; each close marks its
          equity against total-return {run.config.benchmark}. From session {JUDGED_FROM_SESSION} it is judged against the backtest it came from and ranked in the League.
        </p>
      );
    }
    return (
      <p className="run-headline">
        Live for {fmt.integer(stats.sessions)} {stats.sessions === 1 ? "session" : "sessions"} since {run.config.start}:{" "}
        <strong className={tone(stats.excess_cagr)}>
          {stats.excess_cagr >= 0 ? "beats" : "trails"} total-return {run.config.benchmark} by {signedYearly(stats.excess_cagr)}
        </strong>
        {stats.sessions < YEAR_SESSIONS && ` (annualized from ${fmt.integer(stats.sessions)} sessions)`}
        {current && `, ${verdictLabel(current).toLowerCase()}`}.
      </p>
    );
  }
  if (pending) {
    return <p className="run-headline" aria-busy="true" />;
  }
  if (!metrics || metrics.sessions === 0) {
    return null;
  }
  const span = `${run.config.start} to ${run.config.end ?? "now"}`;
  const cagr = metrics.cagr;
  const spyCagr = benchmark?.cagr;
  const excess = cagr === null || cagr === undefined || spyCagr === null || spyCagr === undefined ? undefined : cagr - spyCagr;
  const own = metrics.total_return;
  const spy = benchmark?.total_return;
  const returnGap = own === null || own === undefined || spy === null || spy === undefined ? undefined : own - spy;
  const drawdowns = `max drawdown ${METRICS.max_drawdown.render(metrics.max_drawdown)} vs ${METRICS.max_drawdown.render(benchmark?.max_drawdown)}`;
  return (
    <p className="run-headline">
      {run.finished_at ? "Backtest" : "Backtest so far"}, {span}:{" "}
      {excess !== undefined ? (
        <>
          <strong className={tone(excess)}>
            {excess >= 0 ? "beats" : "trails"} total-return SPY by {signedYearly(excess)}
          </strong>{" "}
          (CAGR {fmt.change(cagr)} vs {fmt.change(spyCagr)}), {drawdowns}.
        </>
      ) : returnGap !== undefined ? (
        <>
          <strong className={tone(returnGap)}>
            {returnGap >= 0 ? "ahead of" : "behind"} total-return SPY by {fmt.change(Math.abs(returnGap)).replace(/^[+-]/, "")}
          </strong>{" "}
          (return {fmt.change(metrics.total_return)} vs {fmt.change(benchmark?.total_return)}; CAGR needs a year), {drawdowns}.
        </>
      ) : (
        <>return {fmt.change(metrics.total_return)}.</>
      )}
      {gate && <GateLine gate={gate} />}
    </p>
  );
}

const GATE_SHOWN = 2;

function GateLine({ gate }: { gate: Schemas["GateCheck"][] }) {
  if (gate.length === 0) {
    return " Passes the screening gate.";
  }
  const shown = gate.slice(0, GATE_SHOWN).map((check) => CHECKS[check]);
  const more = gate.length - shown.length;
  return (
    <>
      {` Fails the screening gate: ${shown.join("; ")}${more > 0 ? ` and ${more} more` : ""}.`}
      {more > 0 && (
        <Info label="Screening gate">
          <ul className="gate-list">
            {gate.map((check) => (
              <li key={check}>{CHECKS[check]}</li>
            ))}
          </ul>
        </Info>
      )}
    </>
  );
}

function Overview({ run, live, symbol, gate, closeHref }: { run: Run; live: Live | undefined; symbol: string | undefined; gate: Schemas["GateCheck"][] | undefined; closeHref: string }) {
  const path = { params: { path: { id: run.id } } };
  const curve = useResource<Curve>(() => must(api.GET("/api/runs/{id}/equity", path)), [run.id]);
  const marks = useResource(() => markEvents(run.id), [run.id]);
  const ongoing = run.mode === "live" && !run.finished_at;
  const sync = useMemo(chartSync, []);
  const pricePanel = useRef<HTMLElement>(null);

  useEffect(() => {
    if (symbol) {
      const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
      pricePanel.current?.scrollIntoView({ block: "start", behavior: reduced ? "auto" : "smooth" });
    }
  }, [symbol]);

  const append = useFrameBatch<SparkPoint>((points) => curve.mutate((current) => ({ ...current, points: appendPoints(current.points, points) })));

  useSpineEvents((event) => {
    if (event.type === "job") {
      if (event.job.name === DAILY_BARS_JOB && event.job.last_outcome === "ok" && ongoing) {
        curve.reload();
      }
      return;
    }
    if (!("run_id" in event) || event.run_id !== run.id) {
      return;
    }
    if (event.type === "run_status") {
      curve.reload();
    } else if (event.type === "equity") {
      append({ at: event.at, equity: event.equity });
    } else if (event.type === "log" && isMark(event.event)) {
      const added = event.event;
      marks.mutate((current) => ((current.at(-1)?.seq ?? 0) < added.seq ? [...current, added] : current));
    }
  });

  const calendar = useMemo(
    () => (curve.data ? monthlyCalendar(curve.data.points, curve.data.benchmark, run.config.start_cash) : undefined),
    [curve.data, run.config.start_cash],
  );
  const metrics = run.metrics ?? curve.data?.interim_metrics;
  const benchmark = curve.data && (run.finished_at || run.mode === "live") ? { symbol: run.config.benchmark, metrics: curve.data.benchmark_metrics } : undefined;

  return (
    <div className="overview">
      <Headline run={run} live={live} metrics={metrics} benchmark={curve.data?.benchmark_metrics} pending={curve.data === undefined && curve.error === undefined} gate={gate} />
      <section className="panel chart-panel" aria-label="Equity">
        <StaleFailure resource={curve} />
        {curve.data ? (
          curve.data.points.length > 0 ? (
            <EquityChart
              runId={run.id}
              points={curve.data.points}
              benchmark={curve.data.benchmark}
              bot={run.bot}
              benchmarkSymbol={run.config.benchmark}
              events={marks.data}
              sync={sync}
              envelope={run.live_stats?.curve}
            />
          ) : live ? (
            <LiveStart live={{ ...live, run }} />
          ) : (
            <p className="empty chart-empty">
              {awaitingData(run) ? "Waiting for market data. Equity is marked once the first bar arrives." : run.finished_at ? "This run recorded no equity." : "No equity yet. The curve draws as the run marks each session."}
            </p>
          )
        ) : curve.error ? (
          <Failure message={curve.error} onRetry={curve.reload} />
        ) : (
          <Loading shape="chart" />
        )}
      </section>
      {symbol && (
        <section ref={pricePanel} className="panel chart-panel price-panel" aria-label={`${symbol} price`}>
          <header className="panel-head">
            <h2>{symbol}</h2>
            <span className="muted">{run.config.timeframe === "1d" ? "Daily" : "1-minute"} candles with this run's fills</span>
            <Info label="Prices on this chart">
              Bars up to the run's clock, adjusted for the splits visible by then, so the latest prices match the positions. A fill before a split sits at its split-adjusted
              price; the legend shows the price it traded at. Hover a candle for its fills and their reasons.
            </Info>
            <a className="btn btn-ghost btn-sm" href={closeHref}>
              Close
            </a>
          </header>
          <SymbolChart key={symbol} runId={run.id} symbol={symbol} sync={sync} />
        </section>
      )}
      {metrics && metrics.sessions > 0 && (
        <section className="panel metrics-panel">
          <h2 className="visually-hidden">Metrics</h2>
          <MetricGroups metrics={metrics} benchmark={benchmark} hedge={run.config.hedge_benchmark} months={calendar?.range} />
        </section>
      )}
      {calendar && <MonthlyReturns calendar={calendar} benchmarkSymbol={run.config.benchmark} />}
      <Setup run={run} />
    </div>
  );
}

function LiveStart({ live }: { live: Live }) {
  const source = live.source;
  return (
    <div className="live-start">
      <div className="live-start-text">
        <h2>No session close yet</h2>
        <p>
          The equity curve starts at the close of {firstClose(live, true) ?? "the next session"}. {sourceSentence(live)}
        </p>
        {source && (
          <a className="btn btn-sm" href={href({ page: "bot", name: live.run.bot, run: source.run_id })}>
            Open backtest #{source.run_id}
          </a>
        )}
      </div>
      {source && (
        <figure className="live-start-chart">
          <Sparkline curve={source.curve} mark={undefined} benchmark={`total-return ${source.benchmark_symbol}`} />
          <figcaption className="muted">
            Backtest #{source.run_id}, {source.period.from} → {source.period.to}, against total-return {source.benchmark_symbol}
          </figcaption>
        </figure>
      )}
    </div>
  );
}

function Setup({ run }: { run: Run }) {
  const params = Object.entries(run.config.params).map(([key, value]): Row => [key, <span className="num">{paramValue(value)}</span>]);
  return (
    <details className="panel setup-panel">
      <summary>
        <h2>Setup</h2>
        <span className="muted">params, costs, risk limits and market</span>
      </summary>
      <div className="setup">
        <section>
          <h3>Params</h3>
          {params.length > 0 ? <KeyValues rows={params} mono /> : <p className="empty">The bot takes no params.</p>}
        </section>
        <section>
          <h3>Costs</h3>
          <KeyValues rows={costRows(run.config.cost)} />
        </section>
        <section>
          <h3>Risk limits</h3>
          <KeyValues rows={riskRows(run.config.risk)} />
        </section>
        <section>
          <h3>Market</h3>
          <KeyValues
            rows={[
              ["Universe", universe(run.config.universe)],
              ["Timeframe", run.config.timeframe],
              ["Wakes", run.config.wake],
              ["Benchmark", run.config.benchmark],
              ...(run.config.hedge_benchmark ? [["Hedge benchmark", run.config.hedge_benchmark] satisfies Row] : []),
              ["Start cash", fmt.money(run.config.start_cash)],
            ]}
          />
        </section>
      </div>
    </details>
  );
}

type SymbolHref = (symbol: string) => string;

type Position = Schemas["PositionView"];
type PositionColumn = "symbol" | "qty" | "cost" | "last" | "value" | "unrealized" | "realized" | "weight";

const POSITION_COLUMNS: SortColumns<Position, PositionColumn> = {
  symbol: { value: (position) => position.symbol },
  qty: { value: (position) => position.qty, descending: true },
  cost: { value: (position) => Number(position.avg_cost), descending: true },
  last: { value: (position) => Number(position.last_price), descending: true },
  value: { value: (position) => Number(position.market_value), descending: true },
  unrealized: { value: (position) => Number(position.unrealized_pnl), descending: true },
  realized: { value: (position) => Number(position.realized_pnl), descending: true },
  weight: { value: (position) => Number(position.weight), descending: true },
};

function total(positions: Position[], field: "market_value" | "unrealized_pnl" | "realized_pnl" | "weight"): number {
  return positions.reduce((sum, position) => sum + Number(position[field]), 0);
}

function Positions({ runId, clock, symbolHref }: { runId: number; clock: string | null | undefined; symbolHref: SymbolHref }) {
  const positions = useResource(() => must(api.GET("/api/runs/{id}/positions", { params: { path: { id: runId } } })), [runId]);
  useSpineEvents((event) => {
    if ((event.type === "order" || event.type === "fill") && event.run_id === runId) {
      positions.reload();
    }
  });
  const sorting = useSort("positions", POSITION_COLUMNS, { key: "value", descending: true });
  const flash = useFlash(positions.data, (position) => position.symbol);
  const held = positions.data ?? [];
  const unrealized = total(held, "unrealized_pnl");
  const realized = total(held, "realized_pnl");
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Positions</h2>
        {held.length > 0 && <span className="muted">{fmt.integer(held.length)} open</span>}
      </header>
      <StaleFailure resource={positions} />
      {!positions.data ? (
        positions.error ? <Failure message={positions.error} onRetry={positions.reload} /> : <Loading shape="table" />
      ) : held.length === 0 ? (
        <p className="empty">No open positions{clock ? ` as of ${fmt.market(clock)}` : ""}: the bot holds only cash.</p>
      ) : (
        <div className="table-wrap table-scroll">
          <table className="table fin">
            <thead>
              <tr>
                <SortHeader sorting={sorting} column="symbol">
                  Symbol
                </SortHeader>
                <SortHeader sorting={sorting} column="qty" numeric>
                  Qty
                </SortHeader>
                <SortHeader sorting={sorting} column="cost" numeric>
                  Avg cost
                </SortHeader>
                <SortHeader sorting={sorting} column="last" numeric>
                  Last
                </SortHeader>
                <SortHeader sorting={sorting} column="value" numeric>
                  Value
                </SortHeader>
                <SortHeader sorting={sorting} column="unrealized" numeric>
                  Unrealized
                </SortHeader>
                <SortHeader sorting={sorting} column="realized" numeric>
                  Realized
                </SortHeader>
                <SortHeader sorting={sorting} column="weight" numeric info={<Info label="Weight">Market value as a share of equity. The total is net exposure: longs minus shorts.</Info>}>
                  Weight
                </SortHeader>
              </tr>
            </thead>
            <tbody onKeyDown={moveBetweenRows}>
              {sorting.order(held).map((position) => (
                <tr key={position.symbol} className="clickable" onClick={openRow(symbolHref(position.symbol))} {...flash.row(position.symbol)}>
                  <td className="sym">
                    <SymbolLink symbol={position.symbol} href={symbolHref} />
                  </td>
                  <td className={`r num ${tone(position.qty) ?? ""}`}>{fmt.integer(position.qty)}</td>
                  <td className="r num">{fmt.price(position.avg_cost)}</td>
                  <td className="r num">{fmt.price(position.last_price)}</td>
                  <td className="r num">{fmt.money(position.market_value)}</td>
                  <td className={`r num ${tone(Number(position.unrealized_pnl)) ?? ""}`}>{fmt.money(position.unrealized_pnl)}</td>
                  <td className={`r num ${tone(Number(position.realized_pnl)) ?? ""}`}>{fmt.money(position.realized_pnl)}</td>
                  <td className="r num">{fmt.percent(position.weight)}</td>
                </tr>
              ))}
            </tbody>
            <tfoot>
              <tr>
                <th scope="row" colSpan={4}>
                  Total
                </th>
                <td className="r num">{fmt.money(total(held, "market_value"))}</td>
                <td className={`r num ${tone(unrealized) ?? ""}`}>{fmt.money(unrealized)}</td>
                <td className={`r num ${tone(realized) ?? ""}`}>{fmt.money(realized)}</td>
                <td className="r num">{fmt.percent(total(held, "weight"))}</td>
              </tr>
            </tfoot>
          </table>
        </div>
      )}
    </section>
  );
}

function requested(size: Schemas["OrderSize"]): string {
  return size.by === "qty" ? `${fmt.integer(size.qty)} sh` : `${fmt.percent(size.weight)} wt`;
}

function orderType(type: Schemas["OrderType"]): string {
  switch (type.type) {
    case "market":
      return "MKT";
    case "limit":
      return `LMT ${fmt.price(type.limit_price)}`;
    case "stop":
      return `STP ${fmt.price(type.stop_price)}`;
    case "trailing_stop": {
      const trail = type.trail.by === "amount" ? fmt.price(type.trail.amount) : fmt.percent(type.trail.percent);
      return type.stop_price ? `TRAIL ${trail} @ ${fmt.price(type.stop_price)}` : `TRAIL ${trail}`;
    }
    case "market_on_open":
      return "MOO";
    case "market_on_close":
      return "MOC";
  }
}

type Order = Schemas["OrderView"];

function withFill(order: Order, qty: number, price: number): Pick<Order, "filled_qty" | "avg_fill_price"> {
  const filled = order.filled_qty + qty;
  const average = (Number(order.avg_fill_price ?? 0) * order.filled_qty + price * qty) / filled;
  return { filled_qty: filled, avg_fill_price: String(average) };
}

type OrderColumn = "symbol" | "side" | "status" | "filled" | "size" | "type" | "fill" | "decided" | "reason";

const ORDER_COLUMNS: SortColumns<Order, OrderColumn> = {
  symbol: { value: (order) => order.symbol },
  side: { value: (order) => (order.qty === 0 ? null : order.side) },
  status: { value: (order) => order.status },
  filled: { value: (order) => order.filled_qty, descending: true },
  size: { value: (order) => (order.requested.by === "qty" ? order.requested.qty : Number(order.requested.weight)), descending: true },
  type: { value: (order) => orderType(order.order_type) },
  fill: { value: (order) => (order.avg_fill_price === null || order.avg_fill_price === undefined ? null : Number(order.avg_fill_price)), descending: true },
  decided: { value: (order) => order.id, descending: true },
  reason: { value: (order) => order.reason },
};

function Orders({ runId, active, symbolHref }: { runId: number; active: boolean; symbolHref: SymbolHref }) {
  const orders = usePaged(
    (after) => must(api.GET("/api/runs/{id}/orders", { params: { path: { id: runId }, query: { limit: PAGE_SIZE, ...(after === undefined ? {} : { after }) } } })),
    [runId],
  );
  const sorting = useSort("orders", ORDER_COLUMNS, { key: "decided", descending: true });
  const flash = useFlash(orders.data?.items, (order) => order.id);
  const [query, setQuery] = useQueryParam("orders");
  const patch = (id: number, update: (order: Order) => Order) => {
    if (!orders.data?.items.some((order) => order.id === id)) {
      orders.reload();
      return;
    }
    orders.mutate((current) => ({ ...current, items: current.items.map((order) => (order.id === id ? update(order) : order)) }));
  };

  useSpineEvents((event) => {
    if (event.type === "order" && event.run_id === runId) {
      const { order_id, status, filled_qty, reject_reason, at } = event.order;
      patch(order_id, (order) => ({ ...order, status, filled_qty, reject_reason: reject_reason ?? null, updated_at: at }));
    } else if (event.type === "fill" && event.run_id === runId) {
      const { order_id, qty, price, at } = event.fill;
      patch(order_id, (order) => (Date.parse(at) > Date.parse(order.updated_at) ? { ...order, ...withFill(order, Math.abs(qty), Number(price)), updated_at: at } : order));
    }
  });
  const loaded = orders.data?.items ?? [];
  const shown = sorting.order(
    loaded.filter((order) => matches(query, order.symbol, order.side, label(order.status), order.reject_reason && label(order.reject_reason), orderType(order.order_type), order.reason)),
  );
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Orders</h2>
        {loaded.length > 0 && (
          <>
            <span className="muted">
              {query.trim() ? `${fmt.integer(shown.length)} of ` : ""}
              {fmt.integer(loaded.length)}
              {orders.loadMore ? "+" : ""} orders
            </span>
            <TableFilter label="Filter orders" value={query} onChange={setQuery} />
          </>
        )}
      </header>
      <StaleFailure resource={orders} />
      {!orders.data ? (
        orders.error ? <Failure message={orders.error} onRetry={orders.reload} /> : <Loading shape="table" />
      ) : loaded.length === 0 ? (
        <p className="empty">{active ? "No orders yet. They appear here the moment the bot places them." : "This run placed no orders."}</p>
      ) : shown.length === 0 ? (
        <p className="empty">No loaded orders match “{query.trim()}”.</p>
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
                <SortHeader sorting={sorting} column="status">
                  Status
                </SortHeader>
                <SortHeader sorting={sorting} column="filled" numeric>
                  Filled
                </SortHeader>
                <SortHeader sorting={sorting} column="size">
                  Size
                </SortHeader>
                <SortHeader sorting={sorting} column="type">
                  Type
                </SortHeader>
                <SortHeader sorting={sorting} column="fill" numeric>
                  Avg fill
                </SortHeader>
                <SortHeader sorting={sorting} column="decided">
                  Decided
                </SortHeader>
                <SortHeader sorting={sorting} column="reason">
                  Reason
                </SortHeader>
              </tr>
            </thead>
            <tbody onKeyDown={moveBetweenRows}>
              {shown.map((order) => (
                <tr key={order.id} className={order.qty === 0 ? "clickable quiet" : "clickable"} onClick={openRow(symbolHref(order.symbol))} {...flash.row(order.id)}>
                  <td className="sym">
                    <SymbolLink symbol={order.symbol} href={symbolHref} />
                  </td>
                  {order.qty === 0 ? <td className="muted">{ABSENT}</td> : <td className={order.side === "buy" ? "up" : "down"}>{order.side}</td>}
                  <td className="nowrap">
                    <StatusPill status={order.status} />
                    {order.reject_reason && <span className="muted"> {label(order.reject_reason)}</span>}
                  </td>
                  <td className="r num nowrap">
                    {fmt.integer(order.filled_qty)}/{fmt.integer(order.qty)}
                  </td>
                  <td className="nowrap">{requested(order.requested)}</td>
                  <td className="nowrap">{orderType(order.order_type)}</td>
                  <td className="r num">{fmt.price(order.avg_fill_price)}</td>
                  <td className="nowrap muted num">{fmt.market(order.decided_at)}</td>
                  <td className="reason">{order.reason}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <LoadMore
        onClick={flash.quietly(orders.loadMore)}
        loaded={loaded.length}
        noun="orders"
      />
    </section>
  );
}

type EventFilter = "all" | "logs" | "system";
type RunEvent = Schemas["RunEvent"];
type EventKind = RunEvent["kind"]["type"];
type EventColumn = "time" | "kind" | "message";

const SYSTEM_KINDS: readonly EventKind[] = ["status", "order_rejected", "risk_breach", "bot_exited", "data_warning"];
const EVENT_KINDS: Record<EventFilter, readonly EventKind[] | undefined> = { all: undefined, logs: ["log"], system: SYSTEM_KINDS };

function describe(kind: Schemas["RunEventKind"]): string {
  switch (kind.type) {
    case "log":
      return kind.message;
    case "status":
      return kind.message ? `${kind.status}: ${kind.message}` : kind.status;
    case "order_rejected":
      return `${kind.symbol} ${kind.client_id} rejected: ${label(kind.reason)}`;
    case "risk_breach":
      return `${label(kind.limit)} ${kind.value} breached ${kind.threshold}`;
    case "bot_exited":
      return kind.code === null || kind.code === undefined ? "bot exited" : `bot exited with code ${kind.code}`;
    case "data_warning":
      return kind.message;
  }
}

function eventKind(event: RunEvent): string {
  return event.kind.type === "log" ? event.kind.level : label(event.kind.type);
}

const EVENT_COLUMNS: SortColumns<RunEvent, EventColumn> = {
  time: { value: (event) => event.seq, descending: true },
  kind: { value: eventKind },
  message: { value: (event) => describe(event.kind) },
};

function Events({ runId }: { runId: number }) {
  const [filter, setFilter] = useState<EventFilter>("all");
  const kinds = EVENT_KINDS[filter];
  const events = usePaged(
    (after) =>
      must(
        api.GET("/api/runs/{id}/events", {
          params: { path: { id: runId }, query: { limit: PAGE_SIZE, ...(kinds ? { kind: [...kinds] } : {}), ...(after === undefined ? {} : { after }) } },
        }),
      ),
    [runId, filter],
  );
  const sorting = useSort("events", EVENT_COLUMNS, { key: "time", descending: true });
  const flash = useFlash(events.data?.items, (event) => event.seq);
  const [query, setQuery] = useQueryParam("events");

  useSpineEvents((event) => {
    if (event.type === "log" && event.run_id === runId && (!kinds || kinds.includes(event.event.kind.type))) {
      events.mutate((current) => {
        const last = current.items.at(-1);
        return current.next === null && (!last || last.seq < event.event.seq) ? { ...current, items: [...current.items, event.event] } : current;
      });
    }
  });

  const loaded = events.data?.items ?? [];
  const shown = sorting.order(loaded.filter((event) => matches(query, eventKind(event), describe(event.kind))));
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Events</h2>
        <Info label="Event times">
          Times are the run's market clock in New York time (ET). Rows marked <span className="event-wall">wall</span> happened off the market clock, before the first bar or
          after the run ended, and show when they were recorded, also in ET.
        </Info>
        <div className="segmented segmented-sm" role="radiogroup" aria-label="Event filter">
          {(["all", "logs", "system"] as const).map((option) => (
            <button key={option} type="button" role="radio" aria-checked={filter === option} className={filter === option ? "active" : undefined} onClick={() => setFilter(option)}>
              {option}
            </button>
          ))}
        </div>
        {loaded.length > 0 && <TableFilter label="Filter events" value={query} onChange={setQuery} />}
      </header>
      <StaleFailure resource={events} />
      {!events.data ? (
        events.error ? <Failure message={events.error} onRetry={events.reload} /> : <Loading shape="table" />
      ) : loaded.length === 0 ? (
        <p className="empty">{filter === "all" ? "No events yet. Bot logs, status changes, rejections and risk breaches show here." : `No ${filter} events.`}</p>
      ) : shown.length === 0 ? (
        <p className="empty">No loaded events match “{query.trim()}”.</p>
      ) : (
        <div className="table-wrap table-scroll">
          <table className="table fin">
            <thead>
              <tr>
                <SortHeader sorting={sorting} column="time">
                  Time
                </SortHeader>
                <SortHeader sorting={sorting} column="kind">
                  Kind
                </SortHeader>
                <SortHeader sorting={sorting} column="message">
                  Message
                </SortHeader>
              </tr>
            </thead>
            <tbody>
              {shown.map((event) => (
                <tr key={event.seq} className={`event-${event.kind.type === "log" ? event.kind.level : event.kind.type}`} {...flash.row(event.seq)}>
                  <td className="muted">
                    <time dateTime={event.clock ?? event.at}>{fmt.market(event.clock ?? event.at)}</time>
                    {!event.clock && <span className="event-wall">wall</span>}
                  </td>
                  <td className="event-kind">{eventKind(event)}</td>
                  <td className="event-text">{describe(event.kind)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <LoadMore
        onClick={flash.quietly(events.loadMore)}
        loaded={loaded.length}
        noun="events"
      />
    </section>
  );
}

function BotOutput({ runId }: { runId: number }) {
  const log = useResource(async () => {
    const pending = api.GET("/api/runs/{id}/logs", { params: { path: { id: runId } }, parseAs: "text" });
    await mustSucceed(pending);
    return (await pending).data ?? "";
  }, [runId]);
  const text = useRef<HTMLPreElement>(null);

  useSpineEvents((event) => {
    if (event.type === "run_status" && event.run_id === runId) {
      log.reload();
    }
  });

  useEffect(() => {
    text.current?.scrollTo({ top: text.current.scrollHeight });
  }, [log.data]);

  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Bot output</h2>
        <Info label="Bot output">What the bot process wrote to stdout and stderr, refreshed when the run changes status.</Info>
      </header>
      <StaleFailure resource={log} />
      {log.data === undefined ? (
        log.error ? <Failure message={log.error} onRetry={log.reload} /> : <Loading />
      ) : log.data === "" ? (
        <p className="empty">No output. Anything the bot prints shows here.</p>
      ) : (
        <pre ref={text} className="process-log-text">
          {log.data}
        </pre>
      )}
    </section>
  );
}
