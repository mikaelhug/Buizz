import {
  AreaSeries,
  ColorType,
  createChart,
  type IChartApi,
  type ISeriesApi,
  LineSeries,
  LineStyle,
  type LineData,
  type UTCTimestamp,
} from "lightweight-charts";
import { useEffect, useRef } from "react";
import type { Schemas } from "../api/client";
import { exchangeWallSeconds, fmt } from "../lib/format";
import { useColorScheme } from "../lib/theme";

type Point = Schemas["SparkPoint"];

function toLinePoint(point: Point): LineData<UTCTimestamp> {
  return { time: exchangeWallSeconds(point.at) as UTCTimestamp, value: Number(point.equity) };
}

function toLine(points: Point[]): LineData<UTCTimestamp>[] {
  const line: LineData<UTCTimestamp>[] = [];
  for (const point of points) {
    const next = toLinePoint(point);
    const previous = line.at(-1);
    if (previous && previous.time === next.time) {
      previous.value = next.value;
    } else if (!previous || previous.time < next.time) {
      line.push(next);
    }
  }
  return line;
}

function underwater(line: LineData<UTCTimestamp>[], peak = Number.NEGATIVE_INFINITY): { line: LineData<UTCTimestamp>[]; peak: number } {
  const drawdown = line.map((point) => {
    peak = Math.max(peak, point.value);
    return { time: point.time, value: peak > 0 ? point.value / peak - 1 : 0 };
  });
  return { line: drawdown, peak };
}

const DRAWDOWN_PANE = 1;
const DRAWDOWN_SHARE = 0.3;

function extendsDrawn(points: Point[], drawn: Point[]): boolean {
  const kept = drawn.length - 1;
  return kept > 0 && points.length >= drawn.length && points[0] === drawn[0] && points[kept - 1] === drawn[kept - 1];
}

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

type Props = {
  runId: number;
  points: Point[];
  benchmark: Point[];
};

export function EquityChart({ runId, points, benchmark }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const chart = useRef<IChartApi | null>(null);
  const equitySeries = useRef<ISeriesApi<"Line"> | null>(null);
  const benchmarkSeries = useRef<ISeriesApi<"Line"> | null>(null);
  const drawdownSeries = useRef<ISeriesApi<"Area"> | null>(null);
  const benchmarkDrawdownSeries = useRef<ISeriesApi<"Line"> | null>(null);
  const drawn = useRef<{ points: Point[]; time: number; peak: number }>({ points: [], time: 0, peak: Number.NEGATIVE_INFINITY });
  const scheme = useColorScheme();

  useEffect(() => {
    if (!host.current) {
      return;
    }
    const created = createChart(host.current, {
      autoSize: true,
      layout: { attributionLogo: false },
      handleScale: { mouseWheel: false },
      handleScroll: { mouseWheel: false, pressedMouseMove: true, horzTouchDrag: true, vertTouchDrag: false },
      rightPriceScale: { borderVisible: false, scaleMargins: { top: 0.14, bottom: 0.06 } },
      timeScale: { borderVisible: false },
    });
    const moneyFormat = { type: "custom", formatter: (value: number) => fmt.money(value), minMove: 0.01 } as const;
    const percentFormat = { type: "custom", formatter: (value: number) => fmt.percent(value), minMove: 0.0001 } as const;
    benchmarkSeries.current = created.addSeries(LineSeries, {
      priceFormat: moneyFormat,
      lineWidth: 1,
      lineStyle: LineStyle.Dashed,
      priceLineVisible: false,
      lastValueVisible: false,
    });
    equitySeries.current = created.addSeries(LineSeries, { lineWidth: 2, priceLineVisible: false, priceFormat: moneyFormat });
    benchmarkDrawdownSeries.current = created.addSeries(
      LineSeries,
      { lineWidth: 1, lineStyle: LineStyle.Dashed, priceLineVisible: false, lastValueVisible: false, priceFormat: percentFormat },
      DRAWDOWN_PANE,
    );
    drawdownSeries.current = created.addSeries(
      AreaSeries,
      { lineWidth: 1, invertFilledArea: true, priceLineVisible: false, priceFormat: percentFormat },
      DRAWDOWN_PANE,
    );
    const [equityPane, drawdownPane] = created.panes();
    equityPane?.setStretchFactor(1 - DRAWDOWN_SHARE);
    drawdownPane?.setStretchFactor(DRAWDOWN_SHARE);
    drawn.current = { points: [], time: 0, peak: Number.NEGATIVE_INFINITY };
    chart.current = created;
    return () => {
      created.remove();
      chart.current = null;
    };
  }, []);

  useEffect(() => {
    chart.current?.applyOptions({
      layout: {
        background: { type: ColorType.Solid, color: "transparent" },
        textColor: cssVar("--muted"),
        fontFamily: cssVar("--font"),
        fontSize: 11,
        panes: { separatorColor: cssVar("--border"), separatorHoverColor: cssVar("--border"), enableResize: false },
      },
      grid: { vertLines: { color: cssVar("--grid") }, horzLines: { color: cssVar("--grid") } },
      crosshair: { vertLine: { labelBackgroundColor: cssVar("--accent") }, horzLine: { labelBackgroundColor: cssVar("--accent") } },
    });
    equitySeries.current?.applyOptions({ color: cssVar("--accent") });
    benchmarkSeries.current?.applyOptions({ color: cssVar("--bench") });
    benchmarkDrawdownSeries.current?.applyOptions({ color: cssVar("--bench") });
    const down = cssVar("--down");
    drawdownSeries.current?.applyOptions({ lineColor: down, topColor: `${down}1a`, bottomColor: `${down}59` });
  }, [scheme]);

  useEffect(() => {
    const line = toLine(benchmark);
    benchmarkSeries.current?.setData(line);
    benchmarkDrawdownSeries.current?.setData(underwater(line).line);
  }, [benchmark]);

  useEffect(() => {
    const series = equitySeries.current;
    const drawdown = drawdownSeries.current;
    if (!series || !drawdown) {
      return;
    }
    let { time, peak } = drawn.current;
    if (extendsDrawn(points, drawn.current.points)) {
      for (const point of points.slice(drawn.current.points.length - 1)) {
        const next = toLinePoint(point);
        if (next.time >= time) {
          const underwaterPoint = underwater([next], peak);
          series.update(next);
          drawdown.update(underwaterPoint.line[0] ?? { time: next.time, value: 0 });
          time = next.time;
          peak = underwaterPoint.peak;
        }
      }
    } else {
      const line = toLine(points);
      const underwaterLine = underwater(line);
      series.setData(line);
      drawdown.setData(underwaterLine.line);
      time = line.at(-1)?.time ?? 0;
      peak = underwaterLine.peak;
    }
    drawn.current = { points, time, peak };
  }, [points]);

  useEffect(() => {
    chart.current?.timeScale().fitContent();
  }, [runId, points.length > 0]);

  return <div ref={host} className="equity-chart" />;
}
