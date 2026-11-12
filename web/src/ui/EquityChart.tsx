import {
  AreaSeries,
  BaselineSeries,
  createChart,
  createSeriesMarkers,
  type AutoscaleInfoProvider,
  type IChartApi,
  type IPriceLine,
  type ISeriesApi,
  type ISeriesMarkersPluginApi,
  LineSeries,
  LineStyle,
  type MouseEventParams,
  type SeriesMarkerBarPosition,
  type SeriesMarkerShape,
  type Time,
  type UTCTimestamp,
} from "lightweight-charts";
import { type KeyboardEvent, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import type { Schemas } from "../api/client";
import { fmt } from "../lib/format";
import { themeColor, useColorScheme } from "../lib/theme";
import { chartOptions, followReducedMotion, followWidth, KEY_HINT, moneyFormat, panZoom, paneTitle, type PaneTitle, themedOptions, themeTitles, USER_INPUTS } from "./chart/base";
import { money, ratioTickmarks, wallDate } from "./chart/format";
import { Legend, type Readout, type Reading, type SeriesKey } from "./chart/Legend";
import {
  addPoint,
  asOf,
  capitalOrigin,
  type Curves,
  DAY_SECONDS,
  curves,
  eventMarks,
  firstFrom,
  isIntraday,
  type Mark,
  type MarkKind,
  type Point,
  type Preset,
  presetStart,
  type Sample,
  presetsWithin,
  wallTime,
} from "./chart/model";
import type { ChartSync, Link } from "./chart/sync";
import { RangePresets, SCALE_MODES, type ScaleMode, ScaleSwitch } from "./chart/Toolbar";
import "../styles/chart.css";

type Props = {
  runId: number;
  points: Point[];
  benchmark: Schemas["SparkPoint"][];
  bot?: string;
  benchmarkSymbol?: string;
  events?: Schemas["RunEvent"][] | undefined;
  sync?: ChartSync | undefined;
  envelope?: Schemas["LivePoint"][] | undefined;
};

const PANE = { equity: 0, drawdown: 1, exposure: 2 } as const;
const STRETCH = { equity: 0.7, drawdown: 0.3, exposure: 0.22 } as const;

const MARK_STYLE: Record<MarkKind, { position: SeriesMarkerBarPosition; shape: SeriesMarkerShape; text: string; token: string }> = {
  stop: { position: "aboveBar", shape: "arrowDown", text: "Stop", token: "--down" },
  breach: { position: "aboveBar", shape: "circle", text: "Risk", token: "--warn" },
  pause: { position: "belowBar", shape: "square", text: "Pause", token: "--muted" },
  resume: { position: "belowBar", shape: "arrowUp", text: "Resume", token: "--accent" },
};

const capAtZero: AutoscaleInfoProvider = (base) => {
  const info = base();
  return info?.priceRange ? { ...info, priceRange: { minValue: Math.min(info.priceRange.minValue, 0), maxValue: 0 } } : info;
};

const floorAtZero: AutoscaleInfoProvider = (base) => {
  const info = base();
  return info?.priceRange ? { ...info, priceRange: { minValue: 0, maxValue: Math.max(info.priceRange.maxValue, 0) } } : info;
};

const ratioFormat = { type: "custom", formatter: (value: number) => fmt.percent(value), tickmarksFormatter: ratioTickmarks, minMove: 0.0001 } as const;

type Model = {
  strategy: Curves;
  benchmark: Curves;
  points: Point[];
  benchmarkPoints: Schemas["SparkPoint"][];
  intraday: boolean;
  marks: Mark[];
  floor: Sample[];
};

type Chart = {
  api: IChartApi;
  equity: ISeriesApi<"Line">;
  benchmark: ISeriesApi<"Line">;
  floor: ISeriesApi<"Line">;
  drawdown: ISeriesApi<"Baseline">;
  benchmarkDrawdown: ISeriesApi<"Line">;
  markers: ISeriesMarkersPluginApi<Time>;
  titles: PaneTitle[];
  exposure?: { series: ISeriesApi<"Area">; title: PaneTitle } | undefined;
  startLine?: IPriceLine | undefined;
};

function emptyModel(): Model {
  return { strategy: curves([]), benchmark: curves([]), points: [], benchmarkPoints: [], intraday: false, marks: [], floor: [] };
}

function extendsDrawn(points: Point[], drawn: Point[]): boolean {
  const kept = drawn.length - 1;
  return kept > 0 && points.length >= drawn.length && points[0] === drawn[0] && points[kept - 1] === drawn[kept - 1];
}

function sampleAt(line: Curves, time: number): Sample | undefined {
  const last = line.equity.at(-1);
  return last && time <= last.time ? asOf(line.equity, time) : undefined;
}

function reading(line: Curves, time: number, from: number | undefined): Reading {
  const sample = sampleAt(line, time);
  if (!sample) {
    return {};
  }
  const base = from === undefined ? line.equity[0] : firstFrom(line.equity, from);
  return {
    value: sample.value,
    change: base && base.time <= time && base.value > 0 ? sample.value / base.value - 1 : undefined,
    drawdown: asOf(line.drawdown, time)?.value,
  };
}

function lastTime(model: Model): number {
  return Math.max(model.strategy.equity.at(-1)?.time ?? Number.NEGATIVE_INFINITY, model.benchmark.equity.at(-1)?.time ?? Number.NEGATIVE_INFINITY);
}

function firstTime(model: Model): number {
  return Math.min(model.strategy.equity[0]?.time ?? Number.POSITIVE_INFINITY, model.benchmark.equity[0]?.time ?? Number.POSITIVE_INFINITY);
}

function summary(model: Model, names: Record<SeriesKey, string>): string {
  const first = firstTime(model);
  const last = lastTime(model);
  if (!Number.isFinite(first) || !Number.isFinite(last)) {
    return `${names.strategy} equity chart, no data yet.`;
  }
  const describe = (key: "strategy" | "benchmark") => {
    const line = model[key];
    const end = line.equity.at(-1);
    const start = line.equity[0];
    if (!end || !start) {
      return `${names[key]} has no data`;
    }
    const worst = line.drawdown.reduce((lowest, sample) => Math.min(lowest, sample.value), 0);
    return `${names[key]} went from ${money(start.value)} to ${money(end.value)} (${fmt.change(end.value / start.value - 1)}), worst drawdown ${fmt.percent(worst)}`;
  };
  return `Equity from ${wallDate(first, model.intraday)} to ${wallDate(last, model.intraday)}. ${describe("strategy")}; ${describe("benchmark")}.`;
}

function floorLine(envelope: Schemas["LivePoint"][] | undefined): Sample[] {
  return (envelope ?? []).flatMap((point) => (point.floor === null || point.floor === undefined ? [] : [{ time: wallTime(point.at), value: Number(point.floor) }]));
}

export function EquityChart({ runId, points, benchmark, bot, benchmarkSymbol, events, sync, envelope }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const chart = useRef<Chart | null>(null);
  const model = useRef<Model>(emptyModel());
  const hover = useRef<number | undefined>(undefined);
  const preset = useRef<Preset | null>("All");
  const userDriven = useRef(false);
  const leading = useRef(false);
  const pointing = useRef(false);
  const link = useRef<Link | undefined>(undefined);
  const [activePreset, setActivePreset] = useState<Preset | null>("All");
  const [presets, setPresets] = useState<Preset[]>([]);
  const [mode, setMode] = useState<ScaleMode>("price");
  const [hidden, setHidden] = useState<Record<SeriesKey, boolean>>({ strategy: false, benchmark: false, floor: false });
  const [exposure, setExposure] = useState(false);
  const [readout, setReadout] = useState<Readout>({ intraday: false, strategy: {}, benchmark: {}, floor: {}, marks: [] });
  const [described, setDescribed] = useState("");
  const scheme = useColorScheme();
  const hintId = useId();
  const floor = useMemo(() => floorLine(envelope), [envelope]);

  const names = useMemo<Record<SeriesKey, string>>(
    () => ({ strategy: bot ?? "Strategy", benchmark: `Total-return ${benchmarkSymbol ?? "benchmark"}`, floor: "Backtest band" }),
    [bot, benchmarkSymbol],
  );

  const refresh = useCallback(() => {
    const current = model.current;
    const time = hover.current ?? current.strategy.equity.at(-1)?.time ?? lastTime(current);
    if (!Number.isFinite(time)) {
      setReadout({ intraday: current.intraday, strategy: {}, benchmark: {}, floor: {}, marks: [] });
      return;
    }
    const range = chart.current?.api.timeScale().getVisibleRange();
    const from = range ? Number(range.from) : undefined;
    const sampled = sampleAt(current.strategy, time)?.time;
    const exposureSample = sampled === undefined ? undefined : asOf(current.strategy.exposure, sampled);
    setReadout({
      time,
      intraday: current.intraday,
      strategy: reading(current.strategy, time, from),
      benchmark: reading(current.benchmark, time, from),
      floor: { value: asOf(current.floor, time)?.value },
      exposure: exposureSample?.time === sampled ? exposureSample?.value : undefined,
      marks: current.marks.filter((mark) => mark.time === sampled),
    });
  }, []);

  const applyPreset = useCallback(
    (next: Preset) => {
      const drawn = chart.current;
      const last = lastTime(model.current);
      userDriven.current = false;
      preset.current = next;
      setActivePreset(next);
      if (!drawn || !Number.isFinite(last)) {
        return;
      }
      for (const pane of drawn.api.panes()) {
        pane.priceScale("right").applyOptions({ autoScale: true });
      }
      const start = presetStart(next, last);
      if (start === undefined) {
        drawn.api.timeScale().fitContent();
      } else {
        drawn.api.timeScale().setVisibleRange({ from: start as UTCTimestamp, to: last as UTCTimestamp });
      }
    },
    [],
  );

  const drawMarks = useCallback(() => {
    const drawn = chart.current;
    if (!drawn) {
      return;
    }
    drawn.markers.setMarkers(
      model.current.marks.map((mark) => {
        const { token, ...style } = MARK_STYLE[mark.kind];
        return { id: mark.id, time: mark.time, color: themeColor(token), ...style };
      }),
    );
  }, []);

  useEffect(() => {
    if (!host.current) {
      return;
    }
    const element = host.current;
    const api = createChart(element, chartOptions((time: Time) => wallDate(Number(time), model.current.intraday)));
    const benchmarkSeries = api.addSeries(LineSeries, {
      priceFormat: moneyFormat,
      lineWidth: 2,
      lineStyle: LineStyle.Dashed,
      priceLineVisible: false,
      lastValueVisible: false,
      crosshairMarkerRadius: 3,
    });
    const floorSeries = api.addSeries(LineSeries, {
      priceFormat: moneyFormat,
      lineWidth: 1,
      lineStyle: LineStyle.Dotted,
      priceLineVisible: false,
      lastValueVisible: false,
      crosshairMarkerVisible: false,
    });
    const equity = api.addSeries(LineSeries, { lineWidth: 2, priceLineVisible: false, priceFormat: moneyFormat, crosshairMarkerRadius: 4 });
    const benchmarkDrawdown = api.addSeries(
      LineSeries,
      { lineWidth: 1, lineStyle: LineStyle.Dashed, priceLineVisible: false, lastValueVisible: false, priceFormat: ratioFormat, autoscaleInfoProvider: capAtZero, crosshairMarkerRadius: 3 },
      PANE.drawdown,
    );
    const drawdown = api.addSeries(
      BaselineSeries,
      {
        baseValue: { type: "price", price: 0 },
        lineWidth: 1,
        priceLineVisible: false,
        priceFormat: ratioFormat,
        autoscaleInfoProvider: capAtZero,
        topLineColor: "transparent",
        topFillColor1: "transparent",
        topFillColor2: "transparent",
        crosshairMarkerRadius: 3,
      },
      PANE.drawdown,
    );
    drawdown.priceScale().applyOptions({ scaleMargins: { top: 0.2, bottom: 0.04 } });
    const [equityPane, drawdownPane] = api.panes();
    equityPane?.setStretchFactor(STRETCH.equity);
    drawdownPane?.setStretchFactor(STRETCH.drawdown);
    const titles = [
      ...(equityPane ? [paneTitle(equityPane, "Equity")] : []),
      ...(drawdownPane ? [paneTitle(drawdownPane, "Drawdown from peak")] : []),
    ];
    chart.current = { api, equity, benchmark: benchmarkSeries, floor: floorSeries, drawdown, benchmarkDrawdown, markers: createSeriesMarkers(equity, []), titles };
    model.current = emptyModel();

    const onCrosshair = (param: MouseEventParams) => {
      hover.current = param.point && param.time !== undefined ? Number(param.time) : undefined;
      if (param.sourceEvent || (hover.current === undefined && pointing.current)) {
        pointing.current = hover.current !== undefined;
        link.current?.crosshair(hover.current);
      }
      refresh();
    };
    const onRange = () => {
      if (userDriven.current && preset.current !== null) {
        preset.current = null;
        setActivePreset(null);
      }
      const range = api.timeScale().getVisibleRange();
      if (leading.current && range) {
        link.current?.span({ from: Number(range.from), to: Number(range.to), preset: preset.current });
      }
      refresh();
    };
    const onDoubleClick = () => applyPreset("All");
    const engage = () => {
      userDriven.current = true;
      leading.current = true;
    };
    const stopMotion = followReducedMotion(api);
    const stopWidth = followWidth(element, () => {
      if (preset.current !== null) {
        applyPreset(preset.current);
      }
    });
    api.subscribeCrosshairMove(onCrosshair);
    api.subscribeDblClick(onDoubleClick);
    api.timeScale().subscribeVisibleLogicalRangeChange(onRange);
    for (const type of USER_INPUTS) {
      element.addEventListener(type, engage, { capture: true, passive: true });
    }
    return () => {
      stopMotion();
      stopWidth();
      for (const type of USER_INPUTS) {
        element.removeEventListener(type, engage, { capture: true });
      }
      api.remove();
      chart.current = null;
    };
  }, [refresh, applyPreset]);

  useEffect(() => {
    const drawn = chart.current;
    if (!drawn || !sync) {
      return;
    }
    const joined = sync.join({
      crosshair: (time) => {
        const sample = time === undefined ? undefined : asOf(model.current.strategy.equity, time);
        hover.current = sample?.time;
        if (sample) {
          drawn.api.setCrosshairPosition(sample.value, sample.time, drawn.equity);
        } else {
          drawn.api.clearCrosshairPosition();
        }
        refresh();
      },
      span: (span) => {
        leading.current = false;
        if (span.preset !== null) {
          applyPreset(span.preset);
          return;
        }
        userDriven.current = false;
        preset.current = null;
        setActivePreset(null);
        drawn.api.timeScale().setVisibleRange({ from: span.from as UTCTimestamp, to: span.to as UTCTimestamp });
      },
    });
    link.current = joined;
    return () => {
      joined.leave();
      link.current = undefined;
    };
  }, [sync, applyPreset, refresh]);

  useEffect(() => {
    const drawn = chart.current;
    if (!drawn || !exposure) {
      return;
    }
    const series = drawn.api.addSeries(
      AreaSeries,
      { lineWidth: 1, priceLineVisible: false, priceFormat: ratioFormat, autoscaleInfoProvider: floorAtZero, crosshairMarkerRadius: 3 },
      PANE.exposure,
    );
    series.setData(model.current.strategy.exposure);
    series.priceScale().applyOptions({ scaleMargins: { top: 0.28, bottom: 0 } });
    const pane = drawn.api.panes()[PANE.exposure];
    pane?.setStretchFactor(STRETCH.exposure);
    drawn.exposure = pane
      ? { series, title: paneTitle(pane, "Gross exposure") }
      : undefined;
    refresh();
    return () => {
      drawn.exposure = undefined;
      if (chart.current === drawn) {
        drawn.api.removePane(PANE.exposure);
      }
    };
  }, [exposure, refresh]);

  useEffect(() => {
    const drawn = chart.current;
    if (!drawn) {
      return;
    }
    const accent = themeColor("--accent");
    const bench = themeColor("--bench");
    const down = themeColor("--down");
    const muted = themeColor("--muted");
    drawn.api.applyOptions(themedOptions());
    drawn.equity.applyOptions({ color: accent, baseLineColor: themeColor("--border-strong") });
    drawn.benchmark.applyOptions({ color: bench, baseLineVisible: false });
    drawn.floor.applyOptions({ color: themeColor("--warn") });
    drawn.benchmarkDrawdown.applyOptions({ color: bench });
    drawn.drawdown.applyOptions({ bottomLineColor: down, bottomFillColor1: themeColor("--down", 0.08), bottomFillColor2: themeColor("--down", 0.32) });
    drawn.exposure?.series.applyOptions({ lineColor: accent, topColor: themeColor("--accent", 0.22), bottomColor: themeColor("--accent", 0.02) });
    drawn.startLine?.applyOptions({ color: muted });
    themeTitles([...drawn.titles, ...(drawn.exposure ? [drawn.exposure.title] : [])]);
    drawMarks();
  }, [scheme, exposure, drawMarks]);

  useEffect(() => {
    const drawn = chart.current;
    if (!drawn) {
      return;
    }
    model.current.floor = floor;
    drawn.floor.setData(floor);
    refresh();
  }, [floor, refresh]);

  useEffect(() => {
    preset.current = "All";
    setActivePreset("All");
  }, [runId]);

  useEffect(() => {
    const drawn = chart.current;
    if (!drawn) {
      return;
    }
    const current = model.current;
    const fresh = benchmark !== current.benchmarkPoints || !extendsDrawn(points, current.points);
    if (fresh) {
      const origin = capitalOrigin(points, benchmark);
      current.strategy = curves(points, origin);
      current.benchmark = curves(benchmark);
      drawn.equity.setData(current.strategy.equity);
      drawn.drawdown.setData(current.strategy.drawdown);
      drawn.exposure?.series.setData(current.strategy.exposure);
      drawn.benchmark.setData(current.benchmark.equity);
      drawn.benchmarkDrawdown.setData(current.benchmark.drawdown);
      if (drawn.startLine) {
        drawn.equity.removePriceLine(drawn.startLine);
        drawn.startLine = undefined;
      }
      if (origin) {
        drawn.startLine = drawn.equity.createPriceLine({
          price: origin.value,
          color: themeColor("--muted"),
          lineWidth: 1,
          lineStyle: LineStyle.Dashed,
          axisLabelVisible: true,
          title: "Start",
        });
      }
      current.intraday = isIntraday(current.strategy.equity);
    } else {
      for (const point of points.slice(current.points.length - 1)) {
        const previous = current.strategy.equity.at(-1);
        const added = addPoint(current.strategy, point);
        if (added) {
          drawn.equity.update(added.equity);
          drawn.drawdown.update(added.drawdown);
          if (added.exposure) {
            drawn.exposure?.series.update(added.exposure);
          }
          current.intraday ||= previous !== undefined && added.equity.time > previous.time && added.equity.time - previous.time < DAY_SECONDS;
        }
      }
    }
    current.points = points;
    current.benchmarkPoints = benchmark;
    drawn.api.applyOptions({ timeScale: { timeVisible: current.intraday } });
    const first = firstTime(current);
    const last = lastTime(current);
    setPresets(Number.isFinite(first) && Number.isFinite(last) ? presetsWithin(first, last) : []);
    if (preset.current !== null) {
      applyPreset(preset.current);
    }
    setDescribed(summary(current, names));
    refresh();
  }, [points, benchmark, names, applyPreset, refresh]);

  useEffect(() => {
    model.current.marks = eventMarks(events ?? [], model.current.strategy.equity);
    drawMarks();
    refresh();
  }, [events, points, benchmark, drawMarks, refresh]);

  useEffect(() => {
    const drawn = chart.current;
    if (!drawn) {
      return;
    }
    drawn.equity.priceScale().applyOptions({ mode: SCALE_MODES[mode] });
    drawn.startLine?.applyOptions({ lineVisible: mode !== "percent" });
  }, [mode]);

  useEffect(() => {
    const drawn = chart.current;
    if (!drawn) {
      return;
    }
    drawn.equity.applyOptions({ visible: !hidden.strategy });
    drawn.drawdown.applyOptions({ visible: !hidden.strategy });
    drawn.benchmark.applyOptions({ visible: !hidden.benchmark });
    drawn.benchmarkDrawdown.applyOptions({ visible: !hidden.benchmark });
    drawn.floor.applyOptions({ visible: !hidden.floor });
  }, [hidden]);

  const pick = (next: Preset) => {
    leading.current = true;
    applyPreset(next);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const timeScale = chart.current?.api.timeScale();
    const range = timeScale?.getVisibleLogicalRange();
    if (!timeScale || !range) {
      return;
    }
    if (event.key === "0") {
      event.preventDefault();
      pick("All");
      return;
    }
    const next = panZoom(event.key, range);
    if (!next || event.altKey || event.ctrlKey || event.metaKey) {
      return;
    }
    event.preventDefault();
    userDriven.current = true;
    timeScale.setVisibleLogicalRange(next);
  };

  return (
    <figure className="equity">
      <div className="chart-toolbar">
        <RangePresets presets={presets} active={activePreset} resetTitle="Show the whole run (double-click the chart or press 0)" onPick={pick} />
        <div className="chart-toolbar-end">
          <ScaleSwitch mode={mode} onChange={setMode} />
          <button
            type="button"
            className={exposure ? "btn btn-sm chart-toggle active" : "btn btn-sm chart-toggle"}
            aria-pressed={exposure}
            title="Gross exposure as a share of equity, in its own pane"
            onClick={() => setExposure((shown) => !shown)}
          >
            Exposure
          </button>
        </div>
      </div>
      <div className={exposure ? "chart-stage chart-stage-tall" : "chart-stage"}>
        <Legend readout={readout} names={names} hidden={hidden} exposure={exposure} envelope={floor.length > 0} onToggle={(key) => setHidden((current) => ({ ...current, [key]: !current[key] }))} />
        <div ref={host} className="equity-chart" tabIndex={0} role="img" aria-label={described} aria-describedby={hintId} onKeyDown={onKeyDown} />
      </div>
      <p id={hintId} className="visually-hidden">
        {KEY_HINT}
      </p>
    </figure>
  );
}
