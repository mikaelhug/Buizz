import {
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
import { fmt } from "../lib/format";
import { useColorScheme } from "../lib/theme";

type Point = { at: string; equity: string };

function toLine(points: Point[]): LineData<UTCTimestamp>[] {
  const line: LineData<UTCTimestamp>[] = [];
  for (const point of points) {
    const time = Math.floor(Date.parse(point.at) / 1000) as UTCTimestamp;
    const previous = line.at(-1);
    const value = Number(point.equity);
    if (previous && previous.time === time) {
      previous.value = value;
    } else if (!previous || previous.time < time) {
      line.push({ time, value });
    }
  }
  return line;
}

function cssVar(name: string): string {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim();
}

type Props = {
  runId: number;
  points: Point[];
  benchmark: Schemas["SparkPoint"][];
};

export function EquityChart({ runId, points, benchmark }: Props) {
  const host = useRef<HTMLDivElement>(null);
  const chart = useRef<IChartApi | null>(null);
  const equitySeries = useRef<ISeriesApi<"Line"> | null>(null);
  const benchmarkSeries = useRef<ISeriesApi<"Line"> | null>(null);
  const scheme = useColorScheme();

  useEffect(() => {
    if (!host.current) {
      return;
    }
    const created = createChart(host.current, {
      autoSize: true,
      handleScale: { mouseWheel: false },
      handleScroll: { mouseWheel: false, pressedMouseMove: true, horzTouchDrag: true, vertTouchDrag: false },
      localization: { priceFormatter: (value: number) => fmt.money(value) },
      rightPriceScale: { borderVisible: false, scaleMargins: { top: 0.14, bottom: 0.06 } },
      timeScale: { borderVisible: false },
    });
    benchmarkSeries.current = created.addSeries(LineSeries, {
      lineWidth: 1,
      lineStyle: LineStyle.Dashed,
      priceLineVisible: false,
      lastValueVisible: false,
    });
    equitySeries.current = created.addSeries(LineSeries, { lineWidth: 2, priceLineVisible: false });
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
        attributionLogo: false,
      },
      grid: { vertLines: { color: cssVar("--grid") }, horzLines: { color: cssVar("--grid") } },
      crosshair: { vertLine: { labelBackgroundColor: cssVar("--accent") }, horzLine: { labelBackgroundColor: cssVar("--accent") } },
    });
    equitySeries.current?.applyOptions({ color: cssVar("--accent") });
    benchmarkSeries.current?.applyOptions({ color: cssVar("--bench") });
  }, [scheme]);

  useEffect(() => {
    benchmarkSeries.current?.setData(toLine(benchmark));
  }, [benchmark]);

  useEffect(() => {
    equitySeries.current?.setData(toLine(points));
  }, [points]);

  useEffect(() => {
    chart.current?.timeScale().fitContent();
  }, [runId, points.length > 0]);

  return <div ref={host} className="equity-chart" />;
}
