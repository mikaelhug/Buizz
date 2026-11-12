import { type KeyboardEvent, type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { api, must, mustSucceed, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { ABSENT, fmt, label, tone } from "../lib/format";
import { BOT_TABS, type BotTab, href, navigate, type Route } from "../lib/route";
import { appendPoints } from "../lib/series";
import { useFrameBatch } from "../lib/useFrameBatch";
import { PAGE_SIZE, usePaged } from "../lib/usePaged";
import { useResource } from "../lib/useResource";
import { EquityChart } from "../ui/EquityChart";
import { Failure, LoadMore, Loading, StaleFailure } from "../ui/Feedback";
import { MetricGroups, MetricValue } from "../ui/Metrics";
import { monthlyCalendar, MonthlyReturns } from "../ui/MonthlyReturns";
import { Info, Popover } from "../ui/Popover";
import { RunBadges, StatusPill } from "../ui/Status";
import { Trades } from "./Trades";

type Run = Schemas["RunSummary"];
type SparkPoint = Schemas["SparkPoint"];
type Curve = Omit<Schemas["EquitySeries"], "points"> & { points: SparkPoint[] };
type Row = [string, ReactNode];

const DAILY_BARS_JOB = "bars_1d";
const PICKER_RUNS = 12;

const TAB_LABELS: Record<BotTab, string> = {
  overview: "Overview",
  trades: "Trades",
  orders: "Orders",
  events: "Positions & events",
  logs: "Logs",
  runs: "Runs",
};

type Props = {
  name: string;
  runId: number;
  pinned: boolean;
  tab: BotTab;
  botHash: string;
  recent: Run[];
  runs: ReactNode;
};

export function RunView({ name, runId, pinned, tab, botHash, recent, runs }: Props) {
  const run = useResource(() => must(api.GET("/api/runs/{id}", { params: { path: { id: runId } } })), [runId]);
  const tabRoute = (target: BotTab): Route => ({ page: "bot", name, ...(pinned ? { run: runId } : {}), tab: target });
  const tabHref = (target: BotTab) => href(tabRoute(target));

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
    }
  });

  if (!run.data) {
    return run.error ? <Failure message={run.error} onRetry={run.reload} /> : <Loading shape="chart" />;
  }
  return (
    <>
      <StaleFailure resource={run} />
      <RunBar run={run.data} name={name} tab={tab} botHash={botHash} recent={recent} logsHref={tabHref("logs")} runsHref={tabHref("runs")} />
      <Tabs tab={tab} routeFor={tabRoute} />
      <div className="tab-panel" role="tabpanel" id="run-tabpanel" aria-labelledby={`tab-${tab}`}>
        {tab === "overview" && <Overview run={run.data} />}
        {tab === "trades" && <Trades runId={runId} positionsHref={tabHref("events")} />}
        {tab === "orders" && <Orders runId={runId} active={!run.data.finished_at} />}
        {tab === "events" && (
          <div className="split split-wide">
            <Positions runId={runId} clock={run.data.clock} />
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

function costRows(cost: Schemas["CostModel"]): Row[] {
  const perShare = Number(cost.slippage_per_share ?? 0);
  return [
    ["Slippage", perShare ? `${cost.slippage_bps} bps + ${dollars(cost.slippage_per_share)} a share` : `${cost.slippage_bps} bps of price`],
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

function reproRows(run: Run, botHash: string): Row[] {
  const repro = run.reproducibility;
  if (!repro) {
    return [];
  }
  return [
    ["Bot code", <Hash value={repro.bot_hash} note={repro.bot_hash === botHash ? undefined : "changed since"} />],
    ["Engine", repro.engine_version],
    ["Seed", String(repro.seed)],
    ["Data through", fmt.market(repro.data_watermark)],
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

type RunBarProps = { run: Run; name: string; tab: BotTab; botHash: string; recent: Run[]; logsHref: string; runsHref: string };

function RunBar({ run, name, tab, botHash, recent, logsHref, runsHref }: RunBarProps) {
  const repro = reproRows(run, botHash);
  return (
    <section className="run-bar" aria-label="Run">
      <div className="run-bar-head">
        <RunPicker run={run} name={name} tab={tab} recent={recent} runsHref={runsHref} />
        <span className="run-mode">{run.mode}</span>
        <StatusPill status={run.status} />
        <RunBadges badges={run.badges} />
        <Info label="Reproducibility">
          {repro.length > 0 ? (
            <>
              <p className="popover-text">The same bot code, engine, seed and data give the same result hash.</p>
              <KeyValues rows={repro} />
            </>
          ) : (
            "Recorded once the run starts: bot code hash, engine version, seed and the data it saw."
          )}
        </Info>
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
            {run.config.cost.slippage_bps} bps · {dollars(run.config.cost.commission_per_share)}/sh
            <Info label="Cost model">
              <KeyValues rows={costRows(run.config.cost)} />
            </Info>
          </dd>
        </div>
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

function Overview({ run }: { run: Run }) {
  const path = { params: { path: { id: run.id } } };
  const curve = useResource<Curve>(() => must(api.GET("/api/runs/{id}/equity", path)), [run.id]);
  const live = run.mode === "live" && !run.finished_at;

  const append = useFrameBatch<SparkPoint>((points) => curve.mutate((current) => ({ ...current, points: appendPoints(current.points, points) })));

  useSpineEvents((event) => {
    if (event.type === "job") {
      if (event.job.name === DAILY_BARS_JOB && event.job.last_outcome === "ok" && live) {
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
    }
  });

  const calendar = useMemo(
    () => (curve.data ? monthlyCalendar(curve.data.points, curve.data.benchmark, run.config.start_cash) : undefined),
    [curve.data, run.config.start_cash],
  );
  const benchmark = curve.data && (run.finished_at || run.mode === "live") ? { symbol: run.config.benchmark, metrics: curve.data.benchmark_metrics } : undefined;

  return (
    <>
      <section className="panel chart-panel" aria-label="Equity">
        <StaleFailure resource={curve} />
        {curve.data ? (
          curve.data.points.length > 0 ? (
            <EquityChart runId={run.id} points={curve.data.points} benchmark={curve.data.benchmark} />
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
      <section className="panel">
        <h2 className="visually-hidden">Metrics</h2>
        <MetricGroups metrics={run.metrics ?? curve.data?.interim_metrics} benchmark={benchmark} months={calendar?.range} />
      </section>
      {calendar && <MonthlyReturns calendar={calendar} benchmarkSymbol={run.config.benchmark} />}
      <Setup run={run} />
    </>
  );
}

function Setup({ run }: { run: Run }) {
  const params = Object.entries(run.config.params).map(([key, value]): Row => [key, <span className="num">{paramValue(value)}</span>]);
  return (
    <section className="panel">
      <h2>Setup</h2>
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
              ["Start cash", fmt.money(run.config.start_cash)],
            ]}
          />
        </section>
      </div>
    </section>
  );
}

function Positions({ runId, clock }: { runId: number; clock: string | null | undefined }) {
  const positions = useResource(() => must(api.GET("/api/runs/{id}/positions", { params: { path: { id: runId } } })), [runId]);
  useSpineEvents((event) => {
    if ((event.type === "order" || event.type === "fill") && event.run_id === runId) {
      positions.reload();
    }
  });
  return (
    <section className="panel">
      <h2>Positions</h2>
      <StaleFailure resource={positions} />
      {!positions.data ? (
        positions.error ? <Failure message={positions.error} onRetry={positions.reload} /> : <Loading shape="table" />
      ) : positions.data.length === 0 ? (
        <p className="empty">No open positions{clock ? ` as of ${fmt.market(clock)}` : ""}: the bot holds only cash.</p>
      ) : (
        <div className="table-wrap table-scroll">
          <table className="table fin">
            <thead>
              <tr>
                <th>Symbol</th>
                <th className="r">Qty</th>
                <th className="r">Avg cost</th>
                <th className="r">Last</th>
                <th className="r">Value</th>
                <th className="r">Unrealized</th>
                <th className="r">Realized</th>
                <th className="r">Weight</th>
              </tr>
            </thead>
            <tbody>
              {positions.data.map((position) => (
                <tr key={position.symbol}>
                  <td className="sym">{position.symbol}</td>
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

function Orders({ runId, active }: { runId: number; active: boolean }) {
  const orders = usePaged(
    (after) => must(api.GET("/api/runs/{id}/orders", { params: { path: { id: runId }, query: { limit: PAGE_SIZE, ...(after === undefined ? {} : { after }) } } })),
    [runId],
  );
  useSpineEvents((event) => {
    if ((event.type === "order" || event.type === "fill") && event.run_id === runId) {
      orders.reload();
    }
  });
  const count = orders.data?.items.length ?? 0;
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Orders</h2>
        {count > 0 && (
          <span className="muted">
            {fmt.integer(count)}
            {orders.loadMore ? "+" : ""} orders
          </span>
        )}
      </header>
      <StaleFailure resource={orders} />
      {!orders.data ? (
        orders.error ? <Failure message={orders.error} onRetry={orders.reload} /> : <Loading shape="table" />
      ) : count === 0 ? (
        <p className="empty">{active ? "No orders yet. They appear here the moment the bot places them." : "This run placed no orders."}</p>
      ) : (
        <div className="table-wrap table-scroll">
          <table className="table fin">
            <thead>
              <tr>
                <th>Symbol</th>
                <th>Side</th>
                <th>Status</th>
                <th className="r">Filled</th>
                <th>Size</th>
                <th>Type</th>
                <th className="r">Avg fill</th>
                <th>Decided</th>
                <th>Reason</th>
              </tr>
            </thead>
            <tbody>
              {orders.data.items.map((order) => (
                <tr key={order.id} className={order.qty === 0 ? "quiet" : undefined}>
                  <td className="sym">{order.symbol}</td>
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
      <LoadMore onClick={orders.loadMore} />
    </section>
  );
}

type EventFilter = "all" | "logs" | "system";

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

function Events({ runId }: { runId: number }) {
  const events = usePaged(
    (after) => must(api.GET("/api/runs/{id}/events", { params: { path: { id: runId }, query: { limit: PAGE_SIZE, ...(after === undefined ? {} : { after }) } } })),
    [runId],
  );
  const [filter, setFilter] = useState<EventFilter>("all");

  useSpineEvents((event) => {
    if (event.type === "log" && event.run_id === runId) {
      events.mutate((current) => {
        const last = current.items.at(-1);
        return current.next === null && (!last || last.seq < event.event.seq) ? { ...current, items: [...current.items, event.event] } : current;
      });
    }
  });

  const items = (events.data?.items ?? []).filter((event) => filter === "all" || (filter === "logs") === (event.kind.type === "log"));
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
      </header>
      <StaleFailure resource={events} />
      {!events.data ? (
        events.error ? <Failure message={events.error} onRetry={events.reload} /> : <Loading shape="table" />
      ) : items.length === 0 ? (
        <p className="empty">{filter === "all" ? "No events yet. Bot logs, status changes, rejections and risk breaches show here." : `No ${filter} events.`}</p>
      ) : (
        <ol className="event-log">
          {items.map((event) => (
            <li key={event.seq} className={`event event-${event.kind.type === "log" ? event.kind.level : event.kind.type}`}>
              <time className="muted" dateTime={event.clock ?? event.at}>
                {fmt.market(event.clock ?? event.at)}
                {!event.clock && <span className="event-wall">wall</span>}
              </time>
              <span className="event-kind">{event.kind.type === "log" ? event.kind.level : label(event.kind.type)}</span>
              <span className="event-text">{describe(event.kind)}</span>
            </li>
          ))}
        </ol>
      )}
      <LoadMore onClick={events.loadMore} />
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
