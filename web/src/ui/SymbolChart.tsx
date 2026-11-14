import {
  type CandlestickData,
  CandlestickSeries,
  createChart,
  createSeriesMarkers,
  type HistogramData,
  HistogramSeries,
  type IChartApi,
  type IPriceLine,
  type ISeriesApi,
  type ISeriesMarkersPluginApi,
  LineStyle,
  type MouseEventParams,
  type SeriesMarker,
  type Time,
  type UTCTimestamp,
} from "lightweight-charts";
import { type KeyboardEvent, useCallback, useEffect, useEffectEvent, useId, useRef, useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { ABSENT, fmt, tone } from "../lib/format";
import { themeColor, useColorScheme } from "../lib/theme";
import { useFrameBatch } from "../lib/useFrameBatch";
import { useResource } from "../lib/useResource";
import { chartOptions, clearTitles, followSize, KEY_HINT, moneyFormat, panZoom, paneTitle, type PaneTitle, themedOptions, themeTitles, USER_INPUTS } from "./chart/base";
import { money, wallDate } from "./chart/format";
import { asOf, DAY_SECONDS, firstFrom, type Preset, presetStart, presetsWithin, wallTime } from "./chart/model";
import type { ChartSync, Link } from "./chart/sync";
import { RangePresets, SCALE_MODES, type ScaleMode, ScaleSwitch } from "./chart/Toolbar";
import { Failure, Loading, StaleFailure } from "./Feedback";
import { brushZoom, fitPrices } from "./chart/zoom";
import "../styles/chart.css";

type PriceChart = Schemas["PriceChart"];
type Fill = Schemas["ChartFill"];
type Candle = CandlestickData<UTCTimestamp> & { volume: number };
type Placed = Fill & { time: UTCTimestamp };

type Props = { runId: number; symbol: string; sync?: ChartSync | undefined };

const MINUTE_SECONDS = 60;
const PAGE_EDGE = 20;
const LABELLED_SPACING = 10;

function markerStride(api: IChartApi): number {
  return Math.max(1, Math.ceil(LABELLED_SPACING / api.timeScale().options().barSpacing));
}

function fillMarkers(fills: Placed[], candles: Candle[], stride: number, buy: string, sell: string): SeriesMarker<Time>[] {
  if (stride === 1) {
    return fills.map((fill) =>
      fill.qty > 0
        ? { id: `fill-${fill.order_id}-${fill.at}`, time: fill.time, position: "atPriceBottom", price: Number(fill.chart_price), shape: "arrowUp", color: buy, text: "B" }
        : { id: `fill-${fill.order_id}-${fill.at}`, time: fill.time, position: "atPriceTop", price: Number(fill.chart_price), shape: "arrowDown", color: sell, text: "S" },
    );
  }
  const index = new Map(candles.map((candle, at) => [candle.time, at]));
  const buckets = new Map<string, SeriesMarker<Time>>();
  for (const fill of fills) {
    const bought = fill.qty > 0;
    const key = `${bought ? "b" : "s"}-${Math.floor((index.get(fill.time) ?? 0) / stride)}`;
    const price = Number(fill.chart_price);
    const seen = buckets.get(key);
    if (!seen) {
      buckets.set(
        key,
        bought
          ? { id: `fills-${key}`, time: fill.time, position: "atPriceBottom", price, shape: "arrowUp", color: buy }
          : { id: `fills-${key}`, time: fill.time, position: "atPriceTop", price, shape: "arrowDown", color: sell },
      );
    } else if (seen.position === "atPriceBottom" || seen.position === "atPriceTop") {
      seen.price = bought ? Math.min(seen.price, price) : Math.max(seen.price, price);
    }
  }
  return [...buckets.values()];
}
const PANE = { price: 0, volume: 1 } as const;
const STRETCH = { price: 0.78, volume: 0.22 } as const;

type Model = {
  daily: boolean;
  bars: Schemas["PriceBar"][];
  candles: Candle[];
  fills: Placed[];
  splits: Schemas["ChartSplit"][];
  position?: Schemas["ChartPosition"] | null | undefined;
};

type Drawn = {
  api: IChartApi;
  candles: ISeriesApi<"Candlestick">;
  volume: ISeriesApi<"Histogram">;
  markers: ISeriesMarkersPluginApi<Time>;
  titles: PaneTitle[];
  entry?: IPriceLine | undefined;
};

type Readout = { daily: boolean; candle?: Candle | undefined; previous?: number | undefined; fills: Placed[] };

function load(runId: number, symbol: string, query: { after?: string; before?: string } = {}): Promise<PriceChart> {
  return must(api.GET("/api/runs/{id}/chart/{symbol}", { params: { path: { id: runId, symbol }, query } }));
}

function barTime(at: string, daily: boolean): UTCTimestamp {
  const wall = wallTime(at);
  return (daily ? wall - (wall % DAY_SECONDS) : wall) as UTCTimestamp;
}

function toMark(time: number, daily: boolean): number {
  return daily ? time + DAY_SECONDS - 1 : time + MINUTE_SECONDS;
}

function fromMark(time: number, daily: boolean): number {
  return daily ? time - (time % DAY_SECONDS) : time - MINUTE_SECONDS;
}

function candle(bar: Schemas["PriceBar"], daily: boolean): Candle {
  return { time: barTime(bar.start, daily), open: bar.open, high: bar.high, low: bar.low, close: bar.close, volume: bar.volume };
}

function sameSplits(left: Schemas["ChartSplit"][], right: Schemas["ChartSplit"][]): boolean {
  return left.length === right.length && left.every((split, index) => split.ex_date === right[index]?.ex_date && split.ratio === right[index]?.ratio);
}

function appended(current: PriceChart, next: PriceChart): PriceChart {
  const bars = current.bars.slice();
  for (const bar of next.bars) {
    if (bars.at(-1)?.start === bar.start) {
      bars[bars.length - 1] = bar;
    } else {
      bars.push(bar);
    }
  }
  return { ...next, bars, older: current.older };
}

function prepended(current: PriceChart, page: PriceChart): PriceChart {
  const first = current.bars[0];
  const earlier = first ? page.bars.filter((bar) => Date.parse(bar.start) < Date.parse(first.start)) : page.bars;
  return { ...current, bars: [...earlier, ...current.bars], older: page.older };
}

function splitText(ratio: string): string {
  const shares = Number(ratio);
  return shares >= 1 ? `${fmt.ratio(shares).replace(/\.00$/, "")}-for-1 split` : `1-for-${fmt.ratio(1 / shares).replace(/\.00$/, "")} reverse split`;
}

function fillText(fill: Fill): string {
  const adjusted = fill.chart_price === fill.price ? "" : ` (${fmt.price(fill.chart_price)} split-adjusted)`;
  return `${fill.qty > 0 ? "Bought" : "Sold"} ${fmt.integer(Math.abs(fill.qty))} @ ${fmt.price(fill.price)}${adjusted}`;
}

function summary(symbol: string, model: Model): string {
  const first = model.candles[0];
  const last = model.candles.at(-1);
  if (!first || !last) {
    return `${symbol} price chart, no bars yet.`;
  }
  const high = Math.max(...model.candles.map((bar) => bar.high));
  const low = Math.min(...model.candles.map((bar) => bar.low));
  const buys = model.fills.filter((fill) => fill.qty > 0).length;
  const splits = model.splits.map((split) => `${splitText(split.ratio)} on ${split.ex_date}`);
  const position = model.position ? ` Holding ${fmt.integer(model.position.qty)} shares at an average ${money(Number(model.position.avg_cost))}.` : "";
  return (
    `${symbol} ${model.daily ? "daily" : "minute"} candles from ${wallDate(first.time, !model.daily)} to ${wallDate(last.time, !model.daily)}: ` +
    `opened at ${money(first.open)}, last close ${money(last.close)} (${fmt.change(last.close / first.open - 1)}), high ${money(high)}, low ${money(low)}. ` +
    `${model.fills.length} fills, ${buys} buys and ${model.fills.length - buys} sells.` +
    (splits.length ? ` Prices are adjusted for the ${splits.join(", ")}.` : "") +
    position
  );
}

function emptyModel(): Model {
  return { daily: true, bars: [], candles: [], fills: [], splits: [] };
}

export function SymbolChart({ runId, symbol, sync }: Props) {
  const chart = useResource(() => load(runId, symbol), [runId, symbol]);
  const host = useRef<HTMLDivElement>(null);
  const drawn = useRef<Drawn | null>(null);
  const model = useRef<Model>(emptyModel());
  const hover = useRef<number | undefined>(undefined);
  const preset = useRef<Preset | null>("All");
  const userDriven = useRef(false);
  const leading = useRef(false);
  const pointing = useRef(false);
  const link = useRef<Link | undefined>(undefined);
  const stride = useRef(1);
  const growing = useRef<"idle" | "busy" | "again">("idle");
  const paging = useRef(false);
  const [pageError, setPageError] = useState<string | undefined>(undefined);
  const [activePreset, setActivePreset] = useState<Preset | null>("All");
  const [presets, setPresets] = useState<Preset[]>([]);
  const [mode, setMode] = useState<ScaleMode>("price");
  const [readout, setReadout] = useState<Readout>({ daily: true, fills: [] });
  const [described, setDescribed] = useState("");
  const scheme = useColorScheme();
  const hintId = useId();

  const refresh = useCallback(() => {
    const { candles, fills, daily } = model.current;
    const shown = hover.current === undefined ? candles.at(-1) : asOf(candles, hover.current);
    if (!shown) {
      setReadout({ daily, fills: [] });
      return;
    }
    const index = candles.indexOf(shown);
    setReadout({ daily, candle: shown, previous: candles[index - 1]?.close, fills: fills.filter((fill) => fill.time === shown.time) });
  }, []);

  const applyPreset = useCallback((next: Preset) => {
    const current = drawn.current;
    const last = model.current.candles.at(-1)?.time;
    userDriven.current = false;
    preset.current = next;
    setActivePreset(next);
    if (!current || last === undefined) {
      return;
    }
    fitPrices(current.api);
    const start = presetStart(next, last);
    if (start === undefined) {
      current.api.timeScale().fitContent();
    } else {
      current.api.timeScale().setVisibleRange({ from: start as UTCTimestamp, to: last });
    }
  }, []);

  const wantsOlder = () => {
    const first = model.current.candles[0]?.time;
    const last = model.current.candles.at(-1)?.time;
    const start = preset.current === null || last === undefined ? undefined : presetStart(preset.current, last);
    return userDriven.current || (start !== undefined && first !== undefined && start < first);
  };

  const loadOlder = useEffectEvent(() => {
    const first = chart.data?.bars[0];
    if (paging.current || !chart.data?.older || !first) {
      return;
    }
    paging.current = true;
    setPageError(undefined);
    load(runId, symbol, { before: first.start })
      .then(
        (page) => chart.mutate((current) => prepended(current, page)),
        (error: unknown) => setPageError(errorMessage(error)),
      )
      .finally(() => {
        paging.current = false;
      });
  });

  const grow = useEffectEvent(() => {
    const current = chart.data;
    const last = current?.bars.at(-1);
    if (!current) {
      return;
    }
    if (!last) {
      chart.reload();
      return;
    }
    if (growing.current !== "idle") {
      growing.current = "again";
      return;
    }
    growing.current = "busy";
    load(runId, symbol, { after: last.start })
      .then(
        (next) => {
          if (next.older || !sameSplits(current.splits, next.splits)) {
            chart.reload();
          } else {
            chart.mutate((latest) => appended(latest, next));
          }
        },
        () => chart.reload(),
      )
      .finally(() => {
        const again = growing.current === "again";
        growing.current = "idle";
        if (again) {
          grow();
        }
      });
  });

  const advance = useFrameBatch<true>(() => grow());

  useSpineEvents((event) => {
    if ("run_id" in event && event.run_id === runId && (event.type === "equity" || event.type === "fill" || event.type === "run_status")) {
      advance(true);
    }
  });

  const drawMarkers = useCallback(() => {
    const current = drawn.current;
    if (!current) {
      return;
    }
    const { candles, fills, splits, position } = model.current;
    const buy = themeColor("--up");
    const sell = themeColor("--down");
    stride.current = markerStride(current.api);
    const fillMarks = fillMarkers(fills, candles, stride.current, buy, sell);
    const splitMarks = splits.flatMap((split): SeriesMarker<Time>[] => {
      const midnight = Date.parse(split.ex_date) / 1000;
      const day = firstFrom(candles, midnight);
      return day && day.time - (day.time % DAY_SECONDS) === midnight
        ? [{ id: `split-${split.ex_date}`, time: day.time, position: "aboveBar", shape: "circle", color: themeColor("--muted"), text: splitText(split.ratio) }]
        : [];
    });
    current.markers.setMarkers([...fillMarks, ...splitMarks].sort((left, right) => Number(left.time) - Number(right.time)));
    if (current.entry) {
      current.candles.removePriceLine(current.entry);
      current.entry = undefined;
    }
    if (position) {
      const { qty, avg_cost } = position;
      current.entry = current.candles.createPriceLine({
        price: Number(avg_cost),
        color: themeColor("--accent"),
        lineWidth: 1,
        lineStyle: LineStyle.Dashed,
        axisLabelVisible: true,
        title: `Avg ${qty > 0 ? "long" : "short"} ${fmt.integer(Math.abs(qty))}`,
      });
    }
  }, []);

  useEffect(() => {
    if (!host.current) {
      return;
    }
    const element = host.current;
    const api = createChart(element, chartOptions((time: Time) => wallDate(Number(time), !model.current.daily)));
    const candles = api.addSeries(CandlestickSeries, { priceFormat: moneyFormat, priceLineVisible: false });
    const volume = api.addSeries(HistogramSeries, { priceFormat: { type: "volume" }, priceLineVisible: false, lastValueVisible: false }, PANE.volume);
    const [pricePane, volumePane] = api.panes();
    pricePane?.setStretchFactor(STRETCH.price);
    volumePane?.setStretchFactor(STRETCH.volume);
    drawn.current = {
      api,
      candles,
      volume,
      markers: createSeriesMarkers(candles, []),
      titles: volumePane ? [paneTitle(volumePane, "Volume", 0)] : [],
    };
    model.current = emptyModel();

    const onCrosshair = (param: MouseEventParams) => {
      hover.current = param.point && param.time !== undefined ? Number(param.time) : undefined;
      if (param.sourceEvent || (hover.current === undefined && pointing.current)) {
        pointing.current = hover.current !== undefined;
        link.current?.crosshair(hover.current === undefined ? undefined : toMark(hover.current, model.current.daily));
      }
      refresh();
    };
    const onRange = () => {
      if (userDriven.current && preset.current !== null) {
        preset.current = null;
        setActivePreset(null);
      }
      const range = api.timeScale().getVisibleRange();
      const logical = api.timeScale().getVisibleLogicalRange();
      if (leading.current && range) {
        const { daily } = model.current;
        link.current?.span({ from: daily ? Number(range.from) : toMark(Number(range.from), daily), to: toMark(Number(range.to), daily), preset: preset.current });
      }
      if (logical && logical.from < PAGE_EDGE && wantsOlder()) {
        loadOlder();
      }
      if (markerStride(api) !== stride.current) {
        drawMarkers();
      }
    };
    const onDoubleClick = () => applyPreset("All");
    const engage = () => {
      userDriven.current = true;
      leading.current = true;
    };
    const stopSize = followSize(
      api,
      element,
      () => {
        if (preset.current !== null) {
          applyPreset(preset.current);
        }
      },
      () => clearTitles(drawn.current?.titles ?? []),
    );
    const stopBrush = brushZoom(api, element);
    api.subscribeCrosshairMove(onCrosshair);
    api.subscribeDblClick(onDoubleClick);
    api.timeScale().subscribeVisibleLogicalRangeChange(onRange);
    for (const type of USER_INPUTS) {
      element.addEventListener(type, engage, { capture: true, passive: true });
    }
    return () => {
      stopSize();
      stopBrush();
      for (const type of USER_INPUTS) {
        element.removeEventListener(type, engage, { capture: true });
      }
      api.remove();
      drawn.current = null;
    };
  }, [refresh, applyPreset, drawMarkers]);

  useEffect(() => {
    const current = drawn.current;
    if (!current || !sync) {
      return;
    }
    const joined = sync.join({
      crosshair: (time) => {
        const shown = time === undefined ? undefined : asOf(model.current.candles, fromMark(time, model.current.daily));
        hover.current = shown?.time;
        if (shown) {
          current.api.setCrosshairPosition(shown.close, shown.time, current.candles);
        } else {
          current.api.clearCrosshairPosition();
        }
        refresh();
      },
      span: (span) => {
        leading.current = false;
        if (span.preset !== null) {
          applyPreset(span.preset);
          return;
        }
        const { daily } = model.current;
        userDriven.current = false;
        preset.current = null;
        setActivePreset(null);
        current.api.timeScale().setVisibleRange({ from: fromMark(span.from, daily) as UTCTimestamp, to: fromMark(span.to, daily) as UTCTimestamp });
      },
    });
    link.current = joined;
    return () => {
      joined.leave();
      link.current = undefined;
    };
  }, [sync, applyPreset, refresh]);

  const drawVolume = useCallback(() => {
    const current = drawn.current;
    if (!current) {
      return;
    }
    const up = themeColor("--up", 0.4);
    const down = themeColor("--down", 0.4);
    current.volume.setData(model.current.candles.map((bar): HistogramData<UTCTimestamp> => ({ time: bar.time, value: bar.volume, color: bar.close >= bar.open ? up : down })));
  }, []);


  useEffect(() => {
    const current = drawn.current;
    if (!current) {
      return;
    }
    current.api.applyOptions(themedOptions());
    current.candles.applyOptions({
      upColor: themeColor("--up"),
      downColor: themeColor("--down"),
      borderUpColor: themeColor("--up"),
      borderDownColor: themeColor("--down"),
      wickUpColor: themeColor("--up"),
      wickDownColor: themeColor("--down"),
    });
    themeTitles(current.titles);
    drawVolume();
    drawMarkers();
  }, [scheme, drawVolume, drawMarkers]);

  useEffect(() => {
    const current = drawn.current;
    const data = chart.data;
    if (!current || !data) {
      return;
    }
    const previous = model.current;
    const daily = data.timeframe === "1d";
    const candles = data.bars.map((bar) => candle(bar, daily));
    const placed: Placed[] = data.fills.flatMap((fill) => {
      const bar = asOf(candles, barTime(fill.at, daily));
      return bar ? [{ ...fill, time: bar.time }] : [];
    });
    model.current = { daily, bars: data.bars, candles, fills: placed, splits: data.splits, position: data.position };
    current.api.applyOptions({ timeScale: { timeVisible: !daily } });
    const added = data.bars.length - previous.bars.length;
    const prepend = added > 0 && previous.bars.length > 0 && data.bars[added] === previous.bars[0];
    const extend = previous.bars.length > 0 && data.bars[0] === previous.bars[0] && added >= 0 && daily === previous.daily;
    if (extend) {
      for (const bar of candles.slice(Math.max(previous.bars.length - 1, 0))) {
        current.candles.update(bar);
      }
      drawVolume();
    } else {
      const logical = current.api.timeScale().getVisibleLogicalRange();
      current.candles.setData(candles);
      drawVolume();
      if (prepend && logical && preset.current === null) {
        current.api.timeScale().setVisibleLogicalRange({ from: logical.from + added, to: logical.to + added });
      }
    }
    drawMarkers();
    const first = candles[0]?.time;
    const last = candles.at(-1)?.time;
    setPresets(first !== undefined && last !== undefined ? presetsWithin(first, last) : []);
    if (preset.current !== null && !(extend && added === 0)) {
      applyPreset(preset.current);
    }
    setDescribed(summary(symbol, model.current));
    refresh();
  }, [chart.data, symbol, applyPreset, refresh, drawVolume, drawMarkers]);

  useEffect(() => {
    drawn.current?.candles.priceScale().applyOptions({ mode: SCALE_MODES[mode] });
  }, [mode]);

  const pick = (next: Preset) => {
    leading.current = true;
    applyPreset(next);
  };

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const timeScale = drawn.current?.api.timeScale();
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

  const empty = chart.data !== undefined && chart.data.bars.length === 0;
  return (
    <figure className="equity">
      <StaleFailure resource={chart} />
      {pageError && <Failure message={pageError} onRetry={() => loadOlder()} />}
      <div className="chart-toolbar">
        <RangePresets presets={presets} active={activePreset} resetTitle="Show every loaded bar (double-click the chart or press 0)" onPick={pick} />
        <div className="chart-toolbar-end">
          <ScaleSwitch mode={mode} onChange={setMode} />
        </div>
      </div>
      <div className="chart-stage">
        {(chart.data === undefined || empty) && (
          <div className="chart-overlay">
            {empty ? (
              <p className="empty chart-empty">No {symbol} bars up to the run's clock.</p>
            ) : chart.error ? (
              <Failure message={chart.error} onRetry={chart.reload} />
            ) : (
              <Loading shape="chart" />
            )}
          </div>
        )}
        <PriceLegend readout={readout} position={chart.data?.position} />
        <div ref={host} className="equity-chart" tabIndex={0} role="img" aria-label={described} aria-describedby={hintId} onKeyDown={onKeyDown} />
      </div>
      <p id={hintId} className="visually-hidden">
        {KEY_HINT}
      </p>
    </figure>
  );
}

export function SymbolLink({ symbol, href }: { symbol: string; href: (symbol: string) => string }) {
  return (
    <a className="sym-link" href={href(symbol)} title={`Chart ${symbol} with this run's fills`}>
      {symbol}
    </a>
  );
}

function PriceLegend({ readout, position }: { readout: Readout; position: Schemas["ChartPosition"] | null | undefined }) {
  const { candle: shown, previous, fills, daily } = readout;
  if (!shown) {
    return null;
  }
  const change = previous === undefined ? undefined : shown.close / previous - 1;
  const values: [string, string][] = [
    ["O", money(shown.open)],
    ["H", money(shown.high)],
    ["L", money(shown.low)],
    ["C", money(shown.close)],
  ];
  return (
    <div className="chart-legend price-legend">
      <div className="legend-head">
        <time className="legend-time">{wallDate(shown.time, !daily)}</time>
        {fills.map((fill) => (
          <span key={`${fill.order_id}-${fill.at}`} className={`legend-note legend-fill ${fill.qty > 0 ? "up" : "down"}`} title={fill.reason ? `${fillText(fill)} · ${fill.reason}` : fillText(fill)}>
            {fillText(fill)}
            {fill.reason && <span className="legend-reason"> · {fill.reason}</span>}
          </span>
        ))}
        {position && (
          <span className="legend-note">
            Avg entry <span className="num">{fmt.price(position.avg_cost)}</span> for <span className="num">{fmt.integer(position.qty)}</span> shares
          </span>
        )}
      </div>
      <div className="legend-ohlc num">
        {values.map(([key, value]) => (
          <span key={key}>
            <span className="legend-key">{key}</span> {value}
          </span>
        ))}
        <span className={tone(change) ?? undefined} title="Change from the previous close">
          {change === undefined ? ABSENT : fmt.change(change)}
        </span>
        <span>
          <span className="legend-key">Vol</span> {fmt.integer(Math.round(shown.volume))}
        </span>
      </div>
    </div>
  );
}
