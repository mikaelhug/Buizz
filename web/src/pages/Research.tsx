import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt, label, paramsText, tone } from "../lib/format";
import { href, navigate } from "../lib/route";
import { BulkBar, CompareButton, SelectAll, SelectBox, useSelection } from "../ui/Selection";
import { useFrameBatch } from "../lib/useFrameBatch";
import { useResource } from "../lib/useResource";
import { useStored } from "../lib/stored";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { Stat } from "../ui/Kit";
import { MetricValue } from "../ui/Metrics";
import { OverflowMenu } from "../ui/Menu";
import { Info } from "../ui/Popover";
import { PresetDates } from "../ui/PresetDates";
import { Sparkline } from "../ui/Sparkline";
import { StatusPill } from "../ui/Status";
import { SUB_PERIODS_ABOUT, SubPeriods } from "../ui/SubPeriods";
import { moveBetweenRows, openRow, SortHeader, type SortColumns, useFlash, useSort } from "../ui/Table";
import { notify } from "../ui/Toaster";
import { CHECKS, GATE_ABOUT } from "../ui/Gate";
import { goLive } from "./BotActions";
import { launchSweep } from "./Launch";
import { isPreset } from "./SweepDialog";
import "../styles/research.css";

type Run = Schemas["RunSummary"];
type PresetResearch = Schemas["PresetResearch"];
type Variant = Schemas["VariantResult"];
type Params = Record<string, unknown>;

const PENDING: ReadonlySet<Schemas["RunStatus"]> = new Set(["queued", "starting", "running"]);

const VOLATILITY_MATCH_ABOUT =
  "Volatility matching scales a variant to total-return SPY's risk before comparing them: its daily returns above T-bills are multiplied by SPY's realised volatility divided by its own, the rest of the capital earns the T-bill rate, and leverage above 1× is charged the run's financing spread on top of the T-bill rate. A variant that only holds less or more market than SPY then shows no edge.";

const LUCK_ABOUT =
  "The best Sharpe that many variants with no edge at all would reach by luck: σ·√(2 ln N), with N the variants ever tried on any code of this bot and σ ≈ √(252 / sessions), the standard error of an annualized Sharpe over the period.";

const NEIGHBOURHOOD_ABOUT =
  "The median Sharpe of the variant and its neighbours on the grid (one step either way on each swept param). A plateau of good neighbours is more likely real than a lone peak.";

function key(params: Params): string {
  return JSON.stringify(params);
}

function changed(params: Params, defaults: Params): Params {
  return Object.fromEntries(Object.entries(params).filter(([name, value]) => key({ v: value }) !== key({ v: defaults[name] })));
}

function variantText(params: Params, defaults: Params): string {
  return paramsText(changed(params, defaults)) || "defaults";
}

function varying(variants: Variant[]): string[] {
  const names = [...new Set(variants.flatMap((variant) => Object.keys(variant.params)))];
  return names.filter((name) => new Set(variants.map((variant) => key({ v: variant.params[name] }))).size > 1);
}

function sharpe(variant: Variant): number | undefined {
  return variant.metrics?.sharpe ?? undefined;
}

function excess(variant: Variant): number | undefined {
  return variant.excess_cagr ?? undefined;
}

const EXCESS_ABOUT =
  "The variant's CAGR minus total-return SPY's (dividends reinvested) over exactly the same sessions, at the risk the variant actually took. The gate also asks for the edge at SPY's volatility.";

function quantile(sorted: number[], q: number): number | undefined {
  if (sorted.length === 0) {
    return undefined;
  }
  const at = (sorted.length - 1) * q;
  const low = Math.floor(at);
  const high = Math.ceil(at);
  return (sorted[low] ?? 0) + ((sorted[high] ?? 0) - (sorted[low] ?? 0)) * (at - low);
}

function shownValue(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

function axisValues(variants: Variant[], name: string): unknown[] {
  const seen = new Map(variants.map((variant) => [key({ v: variant.params[name] }), variant.params[name]]));
  return [...seen.values()].toSorted((a, b) => (typeof a === "number" && typeof b === "number" ? a - b : shownValue(a).localeCompare(shownValue(b))));
}

type Props = { name: string; preset: string | undefined; defaults: Params; live: Run[] };

export function Research({ name, preset: requested, defaults, live }: Props) {
  const research = useResource(() => must(api.GET("/api/bots/{name}/research", { params: { path: { name } } })), [name]);
  const reloadOnce = useFrameBatch<unknown>(() => research.reload());

  useSpineEvents((event) => {
    if (event.type === "run_status" && event.bot === name && event.mode === "backtest") {
      reloadOnce(event);
    }
  });

  if (!research.data) {
    return research.error ? <Failure message={research.error} onRetry={research.reload} /> : <Loading shape="table" />;
  }
  const { trials, presets, gate_preset: gatePreset } = research.data;
  const shown = presets.find((entry) => entry.preset === requested) ?? presets.find((entry) => entry.preset === gatePreset) ?? presets[0];
  const queued = presets.flatMap((entry) => entry.variants).filter((variant) => variant.status === "queued").length;
  const liveVariants = new Set(live.map((run) => key(run.config.params)));
  const sweepPreset = isPreset(shown?.preset) ? shown.preset : "10y";

  const cancelQueued = async () => {
    try {
      const { cancelled } = await must(api.POST("/api/bots/{name}/backtests/cancel", { params: { path: { name } } }));
      notify(`${cancelled} queued ${cancelled === 1 ? "backtest" : "backtests"} cancelled`, "success");
    } catch (error) {
      notify(errorMessage(error));
    }
  };

  return (
    <div className="research">
      <StaleFailure resource={research} />
      <section className="panel research-head" aria-label="Trials">
        <div className="research-trials">
          <span className="research-trials-count">
            <strong className="num">{fmt.integer(trials.variants)}</strong> {trials.variants === 1 ? "variant" : "variants"} tried
            <span className="muted"> · {fmt.integer(trials.runs)} runs</span>
          </span>
          <Info label="Trials">
            {fmt.integer(trials.variants)} trials from {fmt.integer(trials.runs)} runs of {name}, live runs and failed backtests included: {fmt.integer(trials.params)}{" "}
            {trials.params === 1 ? "parameter set" : "parameter sets"}, plus {fmt.integer(trials.variants - trials.params)} for parameter sets run again after {name}'s own
            code changed. Its own code is its folder (bot.toml without the description, notes left out) and the buizz_bots modules it imports, so a rerun after a data
            fix, an engine change or an edit to shared code it does not import is the same trial.
            {trials.unversioned_runs > 0 &&
              ` ${fmt.integer(trials.unversioned_runs)} of the runs predate code versioning and count as one trial per parameter set, joined with that set's later trials.`}{" "}
            The more trials, the higher the best one climbs by luck alone.
          </Info>
        </div>
        <div className="research-actions">
          {queued > 0 && (
            <button type="button" className="btn btn-sm btn-ghost" onClick={() => void cancelQueued()}>
              Cancel {queued} queued
            </button>
          )}
          <button type="button" className="btn btn-sm btn-primary" onClick={() => launchSweep(name, sweepPreset)}>
            Sweep
          </button>
        </div>
      </section>
      {!shown ? (
        <section className="panel detail-empty">
          <h2>No screening backtests yet</h2>
          <p className="muted">
            Sweep a grid of params, or backtest a 1y to 10y preset with default costs. Each variant of the current code shows up here with the screening gate it passes
            or fails.
          </p>
        </section>
      ) : (
        <>
          <nav className="segmented research-presets" aria-label="Period">
            {presets.map((entry) => (
              <a key={entry.preset} href={href({ page: "research", name, preset: entry.preset })} className={entry === shown ? "active" : undefined} aria-current={entry === shown ? "page" : undefined}>
                {entry.preset}
                {entry.preset === gatePreset && <span className="research-gate-mark"> · gate</span>}
              </a>
            ))}
          </nav>
          <div className="research-window">
            <PresetDates preset={shown.preset} className="muted research-period" />
            <BenchmarkFacts research={shown} />
          </div>
          <PresetView key={shown.preset} name={name} research={shown} defaults={defaults} liveVariants={liveVariants} trials={trials.variants} gate={shown.preset === gatePreset} />
        </>
      )}
    </div>
  );
}

function BenchmarkFacts({ research }: { research: PresetResearch }) {
  const measured = research.variants.find((variant) => variant.metrics?.cagr !== null && variant.metrics?.cagr !== undefined && variant.excess_cagr !== null && variant.excess_cagr !== undefined);
  const cagr = measured && (measured.metrics?.cagr ?? 0) - (measured.excess_cagr ?? 0);
  return (
    <p className="benchmark-strip">
      <span className="benchmark-title">
        Benchmark
        <Info label="Benchmark">
          Total-return SPY (dividends reinvested) over exactly this period. Every variant's CAGR vs SPY subtracts this CAGR, so it is stated here once instead of on every row.
        </Info>
      </span>
      <span className="benchmark-chip">
        <span className="benchmark-name">SPY · {research.preset}</span>
        <span className="num">{fmt.change(cagr)}/yr</span>
        <span className="num muted">Sharpe {fmt.ratio(research.benchmark_sharpe)}</span>
        <span className="num muted">max DD {fmt.change(research.benchmark_max_drawdown === null || research.benchmark_max_drawdown === undefined ? undefined : -research.benchmark_max_drawdown)}</span>
      </span>
    </p>
  );
}

type PresetProps = { name: string; research: PresetResearch; defaults: Params; liveVariants: Set<string>; trials: number; gate: boolean };

function PresetView({ name, research, defaults, liveVariants, trials, gate }: PresetProps) {
  const [busy, setBusy] = useState(false);
  const { variants } = research;
  const axes = varying(variants);
  const plateau = variants.find((variant) => variant.run_id === research.plateau);
  const best = variants.filter((variant) => sharpe(variant) !== undefined).toSorted((a, b) => (sharpe(b) ?? 0) - (sharpe(a) ?? 0))[0];

  const live = async (variant: Variant) => {
    setBusy(true);
    try {
      const run = await goLive(name, variant.run_id);
      notify(`${name} live with ${variantText(variant.params, defaults)} · run #${run.id}`, "success");
      navigate({ page: "bot", name, run: run.id });
    } catch (error) {
      notify(`${name}: ${errorMessage(error)}`);
      setBusy(false);
    }
  };

  return (
    <>
      <section className="panel research-suggest" aria-label="Suggested variant">
        {plateau ? (
          <>
            <div className="research-suggest-text">
              <h2>Plateau variant</h2>
              <p>
                <span className="mono">{variantText(plateau.params, defaults)}</span> passes the gate with the best neighbourhood median Sharpe,{" "}
                <span className="num">{fmt.ratio(plateau.neighbourhood_sharpe)}</span> (its own <span className="num">{fmt.ratio(sharpe(plateau))}</span>).
              </p>
            </div>
            <div className="research-suggest-stats">
              <Stat label="CAGR vs SPY" value={fmt.change(excess(plateau))} tone={tone(excess(plateau))} sub={`CAGR ${fmt.change(plateau.metrics?.cagr)}`} />
              <Stat label="Thirds ahead" value={`${plateau.sub_periods.filter((edge) => edge > 0).length} of ${plateau.sub_periods.length}`} sub={<SubPeriods edges={plateau.sub_periods} count={false} />} />
              <Stat label="Max drawdown" value={fmt.change(plateau.metrics?.max_drawdown === null || plateau.metrics?.max_drawdown === undefined ? undefined : -plateau.metrics.max_drawdown)} />
              <div className="research-suggest-chart">
                <Sparkline curve={plateau.curve} mark={undefined} benchmark="total-return SPY" />
              </div>
            </div>
            {liveVariants.has(key(plateau.params)) ? (
              <span className="pill pill-live">live</span>
            ) : (
              <button type="button" className="btn btn-primary" disabled={busy} onClick={() => void live(plateau)}>
                Go live with this variant
              </button>
            )}
          </>
        ) : (
          <p className="muted">No variant passes the screening gate on this period yet, so none is suggested for live paper trading.</p>
        )}
      </section>
      {axes.length >= 1 && axes.length <= 2 && variants.length > 1 && <Heatmap name={name} variants={variants} axes={axes} plateau={research.plateau} />}
      <section className="panel" aria-label="Spread">
        <header className="panel-head">
          <h2>Spread of {variants.length} {variants.length === 1 ? "variant" : "variants"}</h2>
          <span className="muted">
            {gate ? "The screening gate is judged on this period" : "A shorter period than the one the gate is judged on"}
            <Info label="Screening gate">{GATE_ABOUT}</Info>
          </span>
        </header>
        <Spread research={research} trials={trials} />
        {best && (
          <p className="research-best">
            Best Sharpe <strong className="num">{fmt.ratio(sharpe(best))}</strong> at <span className="mono">{variantText(best.params, defaults)}</span>, and its
            neighbourhood median is <strong className="num">{fmt.ratio(best.neighbourhood_sharpe)}</strong>
            <Info label="Neighbourhood median">{NEIGHBOURHOOD_ABOUT}</Info>
          </p>
        )}
      </section>
      <VariantTable name={name} variants={variants} axes={axes} defaults={defaults} plateau={research.plateau} liveVariants={liveVariants} busy={busy} onLive={(variant) => void live(variant)} />
    </>
  );
}

const STRIP = { width: 640, height: 56, pad: 12 };

function Spread({ research, trials }: { research: PresetResearch; trials: number }) {
  const values = research.variants.flatMap((variant) => sharpe(variant) ?? []).toSorted((a, b) => a - b);
  const finished = research.variants.filter((variant) => !PENDING.has(variant.status));
  const passing = research.variants.filter((variant) => variant.failed_checks.length === 0).length;
  const beating = finished.filter((variant) => (excess(variant) ?? 0) > 0).length;
  const [low, median, high] = [quantile(values, 0.25), quantile(values, 0.5), quantile(values, 0.75)];
  const spy = research.benchmark_sharpe ?? undefined;
  const luck = research.luck_sharpe ?? undefined;
  const marks = [...values, spy, luck, 0].filter((value): value is number => value !== undefined);
  const [min, max] = [Math.min(...marks), Math.max(...marks)];
  const span = max - min || 1;
  const x = (value: number) => STRIP.pad + ((value - min) / span) * (STRIP.width - 2 * STRIP.pad);
  const summary = `Sharpe of ${values.length} variants: median ${fmt.ratio(median)}, middle half ${fmt.ratio(low)} to ${fmt.ratio(high)}; ${passing} pass the gate; total-return SPY ${fmt.ratio(spy)}; luck line ${fmt.ratio(luck)}`;

  return (
    <>
      <dl className="facts research-facts">
        <div>
          <dt>Median Sharpe</dt>
          <dd className="num">{fmt.ratio(median)}</dd>
        </div>
        <div>
          <dt>Middle half</dt>
          <dd className="num">
            {fmt.ratio(low)} to {fmt.ratio(high)}
          </dd>
        </div>
        <div>
          <dt>
            Beat SPY's CAGR
            <Info label="CAGR vs SPY">{EXCESS_ABOUT}</Info>
          </dt>
          <dd className="num">
            {beating} of {finished.length}
          </dd>
        </div>
        <div>
          <dt>
            Gate passes
            <Info label="Volatility matching">{VOLATILITY_MATCH_ABOUT}</Info>
          </dt>
          <dd className="num">
            {passing} of {finished.length}
          </dd>
        </div>
        <div>
          <dt>
            Luck line
            <Info label="Luck line">{LUCK_ABOUT}</Info>
          </dt>
          <dd className="num">
            {fmt.ratio(luck)}
            <span className="facts-sub"> from {fmt.integer(trials)} trials</span>
          </dd>
        </div>
      </dl>
      {values.length > 0 && (
        <svg className="spread-strip" viewBox={`0 0 ${STRIP.width} ${STRIP.height}`} role="img" aria-label={summary} preserveAspectRatio="none">
          <line className="spread-axis" x1={STRIP.pad} x2={STRIP.width - STRIP.pad} y1={STRIP.height / 2} y2={STRIP.height / 2} />
          <line className="spread-zero" x1={x(0)} x2={x(0)} y1={8} y2={STRIP.height - 8} />
          {low !== undefined && high !== undefined && <rect className="spread-iqr" x={x(low)} width={Math.max(2, x(high) - x(low))} y={STRIP.height / 2 - 9} height={18} rx={4} />}
          {median !== undefined && <line className="spread-median" x1={x(median)} x2={x(median)} y1={STRIP.height / 2 - 12} y2={STRIP.height / 2 + 12} />}
          {values.map((value, index) => (
            <line key={index} className="spread-dot" x1={x(value)} x2={x(value)} y1={STRIP.height / 2 - 6} y2={STRIP.height / 2 + 6} />
          ))}
          {spy !== undefined && <line className="spread-spy" x1={x(spy)} x2={x(spy)} y1={4} y2={STRIP.height - 4} />}
          {luck !== undefined && <line className="spread-luck" x1={x(luck)} x2={x(luck)} y1={4} y2={STRIP.height - 4} />}
        </svg>
      )}
      {values.length > 0 && (
        <p className="spread-legend" aria-hidden="true">
          <span className="key key-dot">variant</span>
          <span className="key key-iqr">middle half</span>
          <span className="key key-median">median</span>
          <span className="key key-spy">total-return SPY</span>
          <span className="key key-luck">luck line</span>
          <span className="spread-range num">
            {fmt.ratio(min)} … {fmt.ratio(max)}
          </span>
        </p>
      )}
    </>
  );
}

function tint(value: number | undefined, scale: number): string | undefined {
  if (value === undefined || scale === 0) {
    return undefined;
  }
  const strength = Math.round(Math.min(1, Math.abs(value) / scale) * 55);
  return `color-mix(in srgb, var(${value >= 0 ? "--up" : "--down"}) ${strength}%, transparent)`;
}

const HEAT_METRICS = {
  excess: { text: "CAGR vs SPY", value: excess, render: fmt.change },
  sharpe: { text: "Sharpe", value: sharpe, render: fmt.ratio },
} as const;

type HeatMetric = keyof typeof HEAT_METRICS;

const isHeatMetric = (value: unknown): value is HeatMetric => value === "excess" || value === "sharpe";

function Heatmap({ name, variants, axes, plateau }: { name: string; variants: Variant[]; axes: string[]; plateau: number | null | undefined }) {
  const [metric, setMetric] = useStored<HeatMetric>("heatmap-metric", "excess", isHeatMetric);
  const shown = HEAT_METRICS[metric];
  const [across, down] = [axes[0] ?? "", axes[1]];
  const columns = axisValues(variants, across);
  const rows = down === undefined ? [undefined] : axisValues(variants, down);
  const scale = Math.max(0, ...variants.map((variant) => Math.abs(shown.value(variant) ?? 0)));
  const cell = (column: unknown, row: unknown) =>
    variants.find((variant) => key({ v: variant.params[across] }) === key({ v: column }) && (down === undefined || key({ v: variant.params[down] }) === key({ v: row })));

  return (
    <section className="panel" aria-label="Heatmap">
      <header className="panel-head">
        <h2>
          {shown.text} by {down === undefined ? across : `${across} and ${down}`}
        </h2>
        <span className="segmented segmented-sm" role="group" aria-label="Colour by">
          {Object.entries(HEAT_METRICS).map(([option, spec]) => (
            <button key={option} type="button" className={option === metric ? "active" : undefined} aria-pressed={option === metric} onClick={() => isHeatMetric(option) && setMetric(option)}>
              {spec.text}
            </button>
          ))}
        </span>
        <span className="monthly-legend">
          <span>below 0</span>
          <span className="monthly-scale" aria-hidden="true" />
          <span>above 0</span>
        </span>
      </header>
      <div className="table-wrap">
        <table className="table heatmap">
          <thead>
            <tr>
              <th scope="col" className="mono">
                {down === undefined ? "" : `${down} \\ ${across}`}
              </th>
              {columns.map((column) => (
                <th key={key({ v: column })} scope="col" className="r mono">
                  {shownValue(column)}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => (
              <tr key={key({ v: row ?? null })}>
                <th scope="row" className="mono">
                  {down === undefined ? across : shownValue(row)}
                </th>
                {columns.map((column) => {
                  const variant = cell(column, row);
                  const value = variant && shown.value(variant);
                  return (
                    <td key={key({ v: column })} className={variant?.run_id === plateau ? "r plateau" : "r"} style={{ background: tint(value, scale) }}>
                      {variant ? (
                        <a href={href({ page: "bot", name, run: variant.run_id })} title={variant.failed_checks.length === 0 ? "passes the gate" : variant.failed_checks.map((check) => CHECKS[check]).join(", ")}>
                          {value === undefined && PENDING.has(variant.status) ? label(variant.status) : shown.render(value)}
                          {variant.failed_checks.length === 0 && <span className="heat-pass"> ✓</span>}
                        </a>
                      ) : (
                        <span className="muted">—</span>
                      )}
                    </td>
                  );
                })}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}

type TableProps = {
  name: string;
  variants: Variant[];
  axes: string[];
  defaults: Params;
  plateau: number | null | undefined;
  liveVariants: Set<string>;
  busy: boolean;
  onLive: (variant: Variant) => void;
};

const METRIC_COLUMNS = [
  ["sharpe", "Sharpe"],
  ["cagr", "CAGR"],
  ["total_return", "Return"],
  ["max_drawdown", "Max DD"],
  ["trades", "Trades"],
] as const;

function VariantTable({ name, variants, axes, defaults, plateau, liveVariants, busy, onLive }: TableProps) {
  const columns: SortColumns<Variant, string> = {
    run: { value: (variant) => variant.run_id, descending: true },
    params: { value: (variant) => variantText(variant.params, defaults) },
    status: { value: (variant) => variant.status },
    neighbourhood: { value: (variant) => variant.neighbourhood_sharpe, descending: true },
    excess: { value: excess, descending: true },
    thirds: { value: (variant) => (variant.sub_periods.length === 0 ? undefined : variant.sub_periods.filter((edge) => edge > 0).length), descending: true },
    gate: { value: (variant) => variant.failed_checks.length },
    ...Object.fromEntries(axes.map((axis) => [`param:${axis}`, { value: (variant: Variant) => {
      const value = variant.params[axis];
      return typeof value === "number" || typeof value === "string" ? value : shownValue(value);
    } }])),
    ...Object.fromEntries(METRIC_COLUMNS.map(([metric]) => [metric, { value: (variant: Variant) => variant.metrics?.[metric], descending: metric !== "max_drawdown" }])),
  };
  const sorting = useSort(`research-${name}`, columns, { key: "neighbourhood", descending: true });
  const flash = useFlash(variants, (variant) => variant.run_id);
  const ordered = sorting.order(variants);
  const chosen = useSelection(ordered.map((variant) => variant.run_id));
  const statusVaries = new Set(variants.map((variant) => variant.status)).size > 1;

  return (
    <section className="panel" aria-label="Variants">
      <header className="panel-head">
        <h2>Variants</h2>
        <span className="muted">The latest run of each variant on this period</span>
      </header>
      <div className="table-wrap table-scroll">
        <table className="table fin research-table">
          <thead>
            <tr>
              <SortHeader sorting={sorting} column="run" select={<SelectAll selection={chosen} label="Select every variant" />}>
                Run
              </SortHeader>
              {axes.length === 0 ? (
                <SortHeader sorting={sorting} column="params">
                  Params
                </SortHeader>
              ) : (
                axes.map((axis) => (
                  <SortHeader key={axis} sorting={sorting} column={`param:${axis}`} numeric>
                    <span className="mono">{axis}</span>
                  </SortHeader>
                ))
              )}
              {statusVaries && (
                <SortHeader sorting={sorting} column="status">
                  Status
                </SortHeader>
              )}
              <SortHeader sorting={sorting} column="excess" numeric info={<Info label="CAGR vs SPY">{EXCESS_ABOUT}</Info>}>
                CAGR vs SPY
              </SortHeader>
              <th scope="col" className="chart-col">
                vs total-return SPY
              </th>
              <SortHeader sorting={sorting} column="thirds" info={<Info label="Thirds">{SUB_PERIODS_ABOUT}</Info>}>
                Thirds
              </SortHeader>
              {METRIC_COLUMNS.slice(0, 1).map(([metric, text]) => (
                <SortHeader key={metric} sorting={sorting} column={metric} numeric>
                  {text}
                </SortHeader>
              ))}
              <SortHeader sorting={sorting} column="neighbourhood" numeric info={<Info label="Neighbourhood median">{NEIGHBOURHOOD_ABOUT}</Info>}>
                Nbhd Sharpe
              </SortHeader>
              {METRIC_COLUMNS.slice(1).map(([metric, text]) => (
                <SortHeader key={metric} sorting={sorting} column={metric} numeric>
                  {text}
                </SortHeader>
              ))}
              <SortHeader sorting={sorting} column="gate" info={<Info label="Screening gate">{GATE_ABOUT}</Info>}>
                Gate
              </SortHeader>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody onKeyDown={moveBetweenRows}>
            {ordered.map((variant) => {
              const target = href({ page: "bot", name, run: variant.run_id });
              const passes = variant.failed_checks.length === 0;
              const isLive = liveVariants.has(key(variant.params));
              return (
                <tr key={variant.run_id} className={variant.run_id === plateau ? "clickable selected" : "clickable"} onClick={openRow(target)} {...flash.row(variant.run_id)}>
                  <td>
                    <SelectBox selection={chosen} value={variant.run_id} label={`Select run #${variant.run_id}`} />
                    <a href={target}>#{variant.run_id}</a>
                  </td>
                  {axes.length === 0 ? (
                    <td className="mono nowrap">{variantText(variant.params, defaults)}</td>
                  ) : (
                    axes.map((axis) => (
                      <td key={axis} className="r mono">
                        {shownValue(variant.params[axis])}
                      </td>
                    ))
                  )}
                  {statusVaries && (
                    <td>
                      <StatusPill status={variant.status} />
                    </td>
                  )}
                  <td className={`r num headline-num ${tone(excess(variant)) ?? ""}`}>{fmt.change(excess(variant))}</td>
                  <td className="spark-cell chart-col">
                    <Sparkline curve={variant.curve} mark={undefined} benchmark="total-return SPY" />
                  </td>
                  <td>
                    <SubPeriods edges={variant.sub_periods} />
                  </td>
                  <td className="r">
                    <MetricValue metrics={variant.metrics} metric="sharpe" />
                  </td>
                  <td className="r num">{fmt.ratio(variant.neighbourhood_sharpe)}</td>
                  {METRIC_COLUMNS.slice(1).map(([metric]) => (
                    <td key={metric} className="r">
                      <MetricValue metrics={variant.metrics} metric={metric} />
                    </td>
                  ))}
                  <td className="gate-cell">
                    {PENDING.has(variant.status) ? (
                      <span className="muted">pending</span>
                    ) : passes ? (
                      <span className="pill pill-ok">pass</span>
                    ) : (
                      <span className="gate-fails">
                        {variant.failed_checks.length === 1 ? "fails 1 check" : `fails ${variant.failed_checks.length} checks`}
                        <Info label={`Run #${variant.run_id} fails`}>
                          <ul className="gate-list">
                            {variant.failed_checks.map((check) => (
                              <li key={check}>{CHECKS[check]}</li>
                            ))}
                          </ul>
                        </Info>
                      </span>
                    )}
                    {variant.run_id === plateau && <span className="pill pill-plateau">plateau</span>}
                  </td>
                  <td className="actions-cell">
                    {isLive ? (
                      <span className="pill pill-live">live</span>
                    ) : (
                      <span className="bot-actions">
                        {passes && variant.status === "completed" && (
                          <button type="button" className="btn btn-xs" disabled={busy} onClick={() => onLive(variant)} aria-label={`Go live with ${variantText(variant.params, defaults)}`}>
                            Go live
                          </button>
                        )}
                        <OverflowMenu
                          label={`Actions for run #${variant.run_id}`}
                          actions={[
                            { text: `Open run #${variant.run_id}`, run: () => navigate({ page: "bot", name, run: variant.run_id }) },
                            ...(variant.status === "completed" && !busy ? [{ text: `Go live with ${variantText(variant.params, defaults)}`, run: () => onLive(variant) }] : []),
                          ]}
                        />
                      </span>
                    )}
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <div className="bulk-dock">
        <BulkBar selection={chosen} noun={chosen.keys.length === 1 ? "variant" : "variants"}>
          <CompareButton runs={chosen.keys} />
        </BulkBar>
      </div>
    </section>
  );
}
