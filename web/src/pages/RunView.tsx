import { useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { ABSENT, fmt, label, tone } from "../lib/format";
import { appendPoints } from "../lib/series";
import { useFrameBatch } from "../lib/useFrameBatch";
import { PAGE_SIZE, type Paged, usePaged } from "../lib/usePaged";
import { type Resource, useResource } from "../lib/useResource";
import { EquityChart } from "../ui/EquityChart";
import { Failure, LoadMore, Loading } from "../ui/Feedback";
import { MetricGrid } from "../ui/Metrics";
import { RunBadges, StatusPill } from "../ui/Status";

type Run = Schemas["RunSummary"];
type SparkPoint = Schemas["SparkPoint"];
type Curve = Omit<Schemas["EquitySeries"], "points"> & { points: SparkPoint[] };

export function RunView({ runId }: { runId: number }) {
  const path = { params: { path: { id: runId } } };
  const run = useResource(() => must(api.GET("/api/runs/{id}", path)), [runId]);
  const curve = useResource<Curve>(() => must(api.GET("/api/runs/{id}/equity", path)), [runId]);
  const positions = useResource(() => must(api.GET("/api/runs/{id}/positions", path)), [runId]);
  const orders = usePaged(
    (after) => must(api.GET("/api/runs/{id}/orders", { params: { path: { id: runId }, query: { limit: PAGE_SIZE, ...(after === undefined ? {} : { after }) } } })),
    [runId],
  );
  const events = usePaged(
    (after) => must(api.GET("/api/runs/{id}/events", { params: { path: { id: runId }, query: { limit: PAGE_SIZE, ...(after === undefined ? {} : { after }) } } })),
    [runId],
  );

  const appendEquity = useFrameBatch<SparkPoint>((points) => {
    const last = points.at(-1);
    if (!last) {
      return;
    }
    curve.mutate((current) => ({ ...current, points: appendPoints(current.points, points) }));
    run.mutate((current) => ({ ...current, equity: last.equity, clock: last.at }));
  });

  useSpineEvents((event) => {
    if (!("run_id" in event) || event.run_id !== runId) {
      return;
    }
    switch (event.type) {
      case "run_status":
        run.reload();
        curve.reload();
        break;
      case "equity":
        appendEquity({ at: event.at, equity: event.equity });
        break;
      case "order":
      case "fill":
        orders.reload();
        positions.reload();
        break;
      case "log":
        events.mutate((current) => {
          const last = current.items.at(-1);
          return current.next === null && (!last || last.seq < event.event.seq) ? { ...current, items: [...current.items, event.event] } : current;
        });
        break;
    }
  });

  if (!run.data) {
    return run.error ? <Failure message={run.error} onRetry={run.reload} /> : <Loading />;
  }
  return (
    <>
      <RunHeader run={run.data} />
      <section className="panel chart-panel">
        <div className="legend">
          <span className="swatch swatch-equity">{run.data.bot}</span>
          <span className="swatch swatch-bench">{run.data.config.benchmark}</span>
        </div>
        {curve.data ? (
          curve.data.points.length > 0 ? (
            <EquityChart runId={runId} points={curve.data.points} benchmark={curve.data.benchmark} />
          ) : (
            <p className="empty chart-empty">{awaitingData(run.data) ? "Waiting for market data" : "No equity yet"}</p>
          )
        ) : curve.error ? (
          <Failure message={curve.error} onRetry={curve.reload} />
        ) : (
          <div className="equity-chart" />
        )}
      </section>
      <section className="panel">
        <MetricGrid metrics={run.data.metrics} />
      </section>
      <div className="split split-wide">
        <Positions positions={positions} />
        <Events events={events} />
      </div>
      <Orders orders={orders} />
    </>
  );
}

function awaitingData(run: Run): boolean {
  return run.mode === "live" && !run.clock && (run.status === "starting" || run.status === "running");
}

function universe(spec: Schemas["UniverseSpec"]): string {
  return "symbols" in spec ? spec.symbols.join(" ") : `Top ${spec.top_dollar_volume} by dollar volume`;
}

function costTitle(cost: Schemas["CostModel"], risk: Schemas["RiskLimits"]): string {
  return [
    `Commission ${cost.commission_per_share}/sh, min ${cost.commission_min}`,
    `Slippage ${cost.slippage_bps} bps`,
    `ADV cap ${fmt.percent(cost.adv_participation)} of ${cost.adv_sessions} sessions`,
    `Borrow ${fmt.percent(cost.borrow_rate)}`,
    `Max position ${fmt.percent(risk.max_position_weight)}`,
    `Max gross ${fmt.percent(risk.max_gross_exposure)}`,
    `Drawdown kill ${fmt.percent(risk.max_drawdown)}`,
  ].join("\n");
}

function reproTitle(run: Run): string | undefined {
  const repro = run.reproducibility;
  if (!repro) {
    return undefined;
  }
  return [
    `Bot ${repro.bot_hash}`,
    `Engine ${repro.engine_version}`,
    `Seed ${repro.seed}`,
    `Data to ${fmt.market(repro.data_watermark)}`,
    `Result ${run.result_hash ?? ABSENT}`,
  ].join("\n");
}

function RunHeader({ run }: { run: Run }) {
  const params = Object.entries(run.config.params)
    .map(([key, value]) => `${key}=${JSON.stringify(value)}`)
    .join(" ");
  return (
    <section className="run-head">
      <h2 className="help" title={reproTitle(run)}>
        Run #{run.id}
      </h2>
      <span className="muted">{run.mode}</span>
      <StatusPill status={run.status} />
      <RunBadges badges={run.badges} />
      {run.message && <span className="run-message">{run.message}</span>}
      <dl className="facts">
        <div>
          <dt>Period</dt>
          <dd>
            {run.config.start} → {run.config.end ?? "now"}
          </dd>
        </div>
        <div>
          <dt>Clock</dt>
          <dd>
            {awaitingData(run) ? (
              <span className="muted" title="Waiting for market data since the run started (local time)">
                waiting since {fmt.local(run.started_at ?? run.created_at)}
              </span>
            ) : (
              fmt.market(run.clock)
            )}
          </dd>
        </div>
        <div>
          <dt>Start cash</dt>
          <dd className="num">{fmt.money(run.config.start_cash)}</dd>
        </div>
        <div>
          <dt>Equity</dt>
          <dd className="num">{fmt.money(run.equity)}</dd>
        </div>
        <div>
          <dt>Universe</dt>
          <dd className="ellipsis" title={universe(run.config.universe)}>
            {universe(run.config.universe)}
          </dd>
        </div>
        {params && (
          <div>
            <dt>Params</dt>
            <dd className="mono">{params}</dd>
          </div>
        )}
        <div>
          <dt className="help" title={costTitle(run.config.cost, run.config.risk)}>
            Costs
          </dt>
          <dd>{run.config.cost.slippage_bps} bps</dd>
        </div>
      </dl>
    </section>
  );
}

function Positions({ positions }: { positions: Resource<Schemas["PositionView"][]> }) {
  return (
    <section className="panel">
      <h2>Positions</h2>
      {!positions.data ? (
        positions.error ? <Failure message={positions.error} onRetry={positions.reload} /> : <Loading />
      ) : positions.data.length === 0 ? (
        <p className="empty">Flat</p>
      ) : (
        <div className="table-wrap">
          <table className="table">
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
  return type.type === "market" ? "MKT" : `LMT ${fmt.price(type.limit_price)}`;
}

function Orders({ orders }: { orders: Paged<Schemas["OrderView"]> }) {
  return (
    <section className="panel">
      <h2>Orders</h2>
      {!orders.data ? (
        orders.error ? <Failure message={orders.error} onRetry={orders.reload} /> : <Loading />
      ) : orders.data.items.length === 0 ? (
        <p className="empty">No orders</p>
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Decided</th>
                <th>Symbol</th>
                <th>Side</th>
                <th className="r">Filled</th>
                <th>Size</th>
                <th>Type</th>
                <th>Status</th>
                <th className="r">Avg fill</th>
                <th>Reason</th>
              </tr>
            </thead>
            <tbody>
              {orders.data.items.map((order) => (
                <tr key={order.id} className={order.qty === 0 ? "quiet" : undefined}>
                  <td className="nowrap muted" title={order.client_id}>
                    {fmt.market(order.decided_at)}
                  </td>
                  <td className="sym">{order.symbol}</td>
                  {order.qty === 0 ? (
                    <td className="muted">{ABSENT}</td>
                  ) : (
                    <td className={order.side === "buy" ? "up" : "down"}>{order.side}</td>
                  )}
                  <td className="r num nowrap">
                    {fmt.integer(order.filled_qty)}/{fmt.integer(order.qty)}
                  </td>
                  <td className="nowrap">{requested(order.requested)}</td>
                  <td className="nowrap">{orderType(order.order_type)}</td>
                  <td className="nowrap" title={order.session ? `Session ${order.session}` : undefined}>
                    <StatusPill status={order.status} />
                    {order.reject_reason && <span className="muted"> {label(order.reject_reason)}</span>}
                  </td>
                  <td className="r num">{fmt.price(order.avg_fill_price)}</td>
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

function Events({ events }: { events: Paged<Schemas["RunEvent"]> }) {
  const [filter, setFilter] = useState<EventFilter>("all");
  const items = (events.data?.items ?? []).filter(
    (event) => filter === "all" || (filter === "logs") === (event.kind.type === "log"),
  );
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Events</h2>
        <div className="segmented segmented-sm" role="radiogroup" aria-label="Event filter">
          {(["all", "logs", "system"] as const).map((option) => (
            <button
              key={option}
              type="button"
              role="radio"
              aria-checked={filter === option}
              className={filter === option ? "active" : undefined}
              onClick={() => setFilter(option)}
            >
              {option}
            </button>
          ))}
        </div>
      </header>
      {!events.data ? (
        events.error ? <Failure message={events.error} onRetry={events.reload} /> : <Loading />
      ) : items.length === 0 ? (
        <p className="empty">No events</p>
      ) : (
        <ol className="event-log">
          {items.map((event) => (
            <li key={event.seq} className={`event event-${event.kind.type === "log" ? event.kind.level : event.kind.type}`}>
              {event.clock ? (
                <time className="muted" dateTime={event.clock} title={`Local ${fmt.local(event.at)}`}>
                  {fmt.market(event.clock)}
                </time>
              ) : (
                <time className="muted wall" dateTime={event.at} title="Local time">
                  {fmt.local(event.at)}
                </time>
              )}
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
