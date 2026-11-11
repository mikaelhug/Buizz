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
  const drawn = useRef<{ points: Point[]; time: number }>({ points: [], time: 0 });
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
    drawn.current = { points: [], time: 0 };
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
    const series = equitySeries.current;
    if (!series) {
      return;
    }
    let time = drawn.current.time;
    if (extendsDrawn(points, drawn.current.points)) {
      for (const point of points.slice(drawn.current.points.length - 1)) {
        const next = toLinePoint(point);
        if (next.time >= time) {
          series.update(next);
          time = next.time;
        }
      }
    } else {
      const line = toLine(points);
      series.setData(line);
      time = line.at(-1)?.time ?? 0;
    }
    drawn.current = { points, time };
  }, [points]);

  useEffect(() => {
    chart.current?.timeScale().fitContent();
  }, [runId, points.length > 0]);

  return <div ref={host} className="equity-chart" />;
}
