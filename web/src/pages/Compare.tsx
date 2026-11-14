import { createChart, type IChartApi, type ISeriesApi, LineSeries, LineStyle, type MouseEventParams, type Time } from "lightweight-charts";
import { type CSSProperties, type KeyboardEvent, useCallback, useEffect, useId, useMemo, useRef, useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { ABSENT, fmt, paramsText, tone } from "../lib/format";
import { href } from "../lib/route";
import { appendPoints } from "../lib/series";
import { themeColor, useColorScheme } from "../lib/theme";
import { useFrameBatch } from "../lib/useFrameBatch";
import { useResource } from "../lib/useResource";
import { Breadcrumbs } from "../ui/Breadcrumbs";
import { chartOptions, KEY_HINT, panZoom, themedOptions, USER_INPUTS } from "../ui/chart/base";
import { change, changeTickmarks, wallDate } from "../ui/chart/format";
import { asOf, isIntraday, type Line, wallTime } from "../ui/chart/model";
import { RangePresets } from "../ui/chart/Toolbar";
import { brushZoom, fitPrices, showSpan } from "../ui/chart/zoom";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { METRICS, type MetricKey, type Metrics } from "../ui/Metrics";
import { Info } from "../ui/Popover";
import { RunBadges, StatusPill } from "../ui/Status";
import "../styles/chart.css";
import "../styles/compare.css";

type Run = Schemas["RunSummary"];
type Point = Schemas["SparkPoint"];
type Compared = { run: Run; points: Point[]; benchmark: Point[]; metrics: Metrics | null | undefined; benchmarkMetrics: Metrics | null | undefined };

const SERIES_TOKENS = ["--series-1", "--series-2", "--series-3", "--series-4"] as const;

type Better = "high" | "low" | undefined;
type Row = { key: MetricKey; better: Better };

const ROWS: Row[] = [
  { key: "total_return", better: "high" },
  { key: "cagr", better: "high" },
  { key: "sharpe", better: "high" },
  { key: "sortino", better: "high" },
  { key: "max_drawdown", better: "high" },
  { key: "volatility", better: "low" },
  { key: "calmar", better: "high" },
  { key: "alpha", better: "high" },
  { key: "beta", better: undefined },
  { key: "win_rate", better: "high" },
  { key: "profit_factor", better: "high" },
  { key: "exposure", better: undefined },
  { key: "turnover", better: undefined },
  { key: "trades", better: undefined },
  { key: "sessions", better: undefined },
];

async function load(id: number): Promise<Compared> {
  const path = { params: { path: { id } } };
  const [run, curve] = await Promise.all([must(api.GET("/api/runs/{id}", path)), must(api.GET("/api/runs/{id}/equity", path))]);
  return { run, points: curve.points, benchmark: curve.benchmark, metrics: run.metrics ?? curve.interim_metrics, benchmarkMetrics: curve.benchmark_metrics };
}

function returns(points: Point[]): Line {
  const base = Number(points[0]?.equity);
  return base ? points.map((point) => ({ time: wallTime(point.at), value: Number(point.equity) / base - 1 })) : [];
}

function excessCagr(entry: Compared): number | undefined {
  const own = entry.metrics?.cagr;
  const bench = entry.benchmarkMetrics?.cagr;
  return own === null || own === undefined || bench === null || bench === undefined ? undefined : own - bench;
}

function best(values: (number | undefined)[], better: Better): number | undefined {
  const present = values.filter((value): value is number => value !== undefined);
  if (!better || present.length < 2 || new Set(present).size < 2) {
    return undefined;
  }
  return better === "high" ? Math.max(...present) : Math.min(...present);
}

function delta(value: number | undefined, reference: number | undefined, key: MetricKey | "excess"): string | undefined {
  if (value === undefined || reference === undefined || value === reference) {
    return undefined;
  }
  const diff = value - reference;
  if (key === "trades" || key === "sessions") {
    return `${diff > 0 ? "+" : ""}${fmt.integer(diff)}`;
  }
  if (key !== "excess" && METRICS[key].render === fmt.ratio) {
    return `${diff > 0 ? "+" : ""}${fmt.ratio(diff)}`;
  }
  return `${diff > 0 ? "+" : ""}${(diff * 100).toFixed(2)} pp`;
}

function chip(index: number): CSSProperties {
  return { "--chip": `var(${SERIES_TOKENS[index] ?? "--accent"})` } as CSSProperties;
}

function label(run: Run): string {
  return `${run.bot} #${run.id}`;
}

export function Compare({ runs: ids }: { runs: number[] }) {
  const compared = useResource(() => Promise.all(ids.map(load)), [ids.join(",")]);
  const reloadOnce = useFrameBatch<unknown>(() => compared.reload());
  const append = useFrameBatch<{ id: number; point: Point }>((added) =>
    compared.mutate((current) =>
      current.map((entry) => {
        const points = added.filter((item) => item.id === entry.run.id).map((item) => item.point);
        return points.length > 0 ? { ...entry, points: appendPoints(entry.points, points) } : entry;
      }),
    ),
  );

  useSpineEvents((event) => {
    if (!("run_id" in event) || !ids.includes(event.run_id)) {
      return;
    }
    if (event.type === "run_status" || event.type === "live_session") {
      reloadOnce(event);
    } else if (event.type === "equity") {
      append({ id: event.run_id, point: { at: event.at, equity: event.equity } });
    }
  });

  const bots = [...new Set(compared.data?.map((entry) => entry.run.bot) ?? [])];
  const trail = [{ text: "Bots", href: href({ page: "bots" }) }, ...(bots.length === 1 && bots[0] ? [{ text: bots[0], href: href({ page: "bot", name: bots[0] }) }] : []), { text: "Compare" }];

  return (
    <div className="compare">
      <header className="page-head page-head-stacked">
        <Breadcrumbs trail={trail} />
        <h1>Compare runs</h1>
      </header>
      {ids.length < 2 ? (
        <p className="panel muted compare-empty">Select two to four runs in a bot's history or research variants, or live runs on the Bots page, and choose Compare.</p>
      ) : !compared.data ? (
        compared.error ? (
          <Failure message={compared.error} onRetry={compared.reload} />
        ) : (
          <Loading shape="chart" />
        )
      ) : (
        <>
          <StaleFailure resource={compared} />
          <Chips entries={compared.data} ids={ids} />
          <section className="panel chart-panel" aria-label="Returns">
            <header className="panel-head">
              <h2>Return since each run's start</h2>
              <Info label="Compared returns">
                Each run's equity as a change from its own first mark, so runs with different capital or periods line up. The dashed line is total-return {compared.data[0]?.run.config.benchmark}{" "}
                (dividends reinvested) over the first run's period.
              </Info>
            </header>
            <CompareChart entries={compared.data} />
          </section>
          <MetricTable entries={compared.data} />
        </>
      )}
    </div>
  );
}

function Chips({ entries, ids }: { entries: Compared[]; ids: number[] }) {
  return (
    <ul className="compare-chips">
      {entries.map((entry, index) => {
        const rest = ids.filter((id) => id !== entry.run.id);
        return (
          <li key={entry.run.id} className="compare-chip" style={chip(index)}>
            <span className="compare-swatch" aria-hidden="true" />
            <a className="compare-chip-name" href={href({ page: "bot", name: entry.run.bot, run: entry.run.id })}>
              {label(entry.run)}
            </a>
            <span className="muted">
              {entry.run.mode} · {entry.run.config.start} → {entry.run.config.end ?? "now"}
            </span>
            <StatusPill status={entry.run.status} />
            <a className="compare-remove" href={href({ page: "compare", runs: rest })} aria-label={`Remove ${label(entry.run)} from the comparison`}>
              ×
            </a>
          </li>
        );
      })}
    </ul>
  );
}

type Readout = { time: number | undefined; values: (number | undefined)[]; benchmark: number | undefined };

function CompareChart({ entries }: { entries: Compared[] }) {
  const host = useRef<HTMLDivElement>(null);
  const chart = useRef<{ api: IChartApi; lines: ISeriesApi<"Line">[]; benchmark: ISeriesApi<"Line"> } | null>(null);
  const scheme = useColorScheme();
  const lines = useMemo(() => entries.map((entry) => returns(entry.points)), [entries]);
  const benchmark = useMemo(() => returns(entries[0]?.benchmark ?? []), [entries]);
  const intraday = lines.some(isIntraday);
  const [readout, setReadout] = useState<Readout>({ time: undefined, values: [], benchmark: undefined });
  const [zoomed, setZoomed] = useState(false);
  const userDriven = useRef(false);
  const hintId = useId();
  const latest = useRef({ lines, benchmark });
  latest.current = { lines, benchmark };

  const read = (time: number | undefined): Readout => {
    const at = (line: Line) => (time === undefined ? line.at(-1) : asOf(line, time))?.value;
    return { time, values: latest.current.lines.map(at), benchmark: at(latest.current.benchmark) };
  };

  const showAll = useCallback(() => {
    const drawn = chart.current;
    userDriven.current = false;
    setZoomed(false);
    const all = [...latest.current.lines, latest.current.benchmark];
    const first = Math.min(...all.flatMap((line) => (line[0] ? [line[0].time] : [])));
    const last = Math.max(...all.flatMap((line) => line.slice(-1).map((sample) => sample.time)));
    if (drawn && Number.isFinite(first) && Number.isFinite(last)) {
      fitPrices(drawn.api);
      showSpan(drawn.api, first, last);
    }
  }, []);

  useEffect(() => {
    const element = host.current;
    if (!element) {
      return;
    }
    const format = { type: "custom", formatter: (value: number) => change(value * 100), tickmarksFormatter: (values: number[]) => changeTickmarks(values.map((value) => value * 100)), minMove: 0.0001 } as const;
    const api = createChart(element, chartOptions((time: Time) => wallDate(Number(time), intraday)));
    const benchmarkLine = api.addSeries(LineSeries, { lineWidth: 2, lineStyle: LineStyle.Dashed, priceLineVisible: false, lastValueVisible: false, priceFormat: format });
    const series = entries.map(() => api.addSeries(LineSeries, { lineWidth: 2, priceLineVisible: false, priceFormat: format, crosshairMarkerRadius: 3 }));
    chart.current = { api, lines: series, benchmark: benchmarkLine };
    userDriven.current = false;
    const onCrosshair = (param: MouseEventParams) => setReadout(read(param.point && param.time !== undefined ? Number(param.time) : undefined));
    const onRange = () => {
      if (userDriven.current) {
        setZoomed(true);
      }
    };
    const engage = () => {
      userDriven.current = true;
    };
    api.subscribeCrosshairMove(onCrosshair);
    const onSize = () => {
      if (!userDriven.current) {
        requestAnimationFrame(showAll);
      }
    };
    api.subscribeDblClick(showAll);
    api.timeScale().subscribeVisibleLogicalRangeChange(onRange);
    api.timeScale().subscribeSizeChange(onSize);
    for (const type of USER_INPUTS) {
      element.addEventListener(type, engage, { capture: true, passive: true });
    }
    const stopBrush = brushZoom(api, element);
    return () => {
      stopBrush();
      for (const type of USER_INPUTS) {
        element.removeEventListener(type, engage, { capture: true });
      }
      api.remove();
      chart.current = null;
    };
  }, [entries.length, intraday, showAll]);

  useEffect(() => {
    const drawn = chart.current;
    if (!drawn) {
      return;
    }
    drawn.api.applyOptions(themedOptions());
    drawn.benchmark.applyOptions({ color: themeColor("--bench") });
    drawn.lines.forEach((line, index) => line.applyOptions({ color: themeColor(SERIES_TOKENS[index] ?? "--accent") }));
  }, [scheme, entries.length, intraday]);

  useEffect(() => {
    const drawn = chart.current;
    if (!drawn) {
      return;
    }
    drawn.benchmark.setData(benchmark);
    drawn.lines.forEach((line, index) => line.setData(lines[index] ?? []));
    if (!userDriven.current) {
      showAll();
    }
    setReadout(read(undefined));
  }, [lines, benchmark, showAll]);

  const onKeyDown = (event: KeyboardEvent<HTMLDivElement>) => {
    const timeScale = chart.current?.api.timeScale();
    const range = timeScale?.getVisibleLogicalRange();
    if (!timeScale || !range || event.altKey || event.ctrlKey || event.metaKey) {
      return;
    }
    if (event.key === "0") {
      event.preventDefault();
      showAll();
      return;
    }
    const next = panZoom(event.key, range);
    if (next) {
      event.preventDefault();
      timeScale.setVisibleLogicalRange(next);
    }
  };

  const description = entries.map((entry, index) => `${label(entry.run)} ${fmt.change(lines[index]?.at(-1)?.value)}`).join(", ");

  return (
    <div className="compare-chart">
      <div className="chart-toolbar">
        <RangePresets presets={[]} active={zoomed ? null : "All"} resetTitle="Show every run's whole period (double-click the chart)" onPick={showAll} />
      </div>
      <div className="chart-legend compare-legend" aria-hidden="true">
        <div className="legend-head">
          <span className="legend-time">{readout.time === undefined ? "Latest" : wallDate(readout.time, intraday)}</span>
        </div>
        {entries.map((entry, index) => (
          <span key={entry.run.id} className="legend-row" style={chip(index)}>
            <span className="compare-swatch" />
            <span className="legend-name">{label(entry.run)}</span>
            <span className={`num ${tone(readout.values[index]) ?? ""}`}>{fmt.change(readout.values[index])}</span>
          </span>
        ))}
        <span className="legend-row">
          <span className="legend-swatch legend-swatch-benchmark" />
          <span className="legend-name">Total-return {entries[0]?.run.config.benchmark}</span>
          <span className={`num ${tone(readout.benchmark) ?? ""}`}>{fmt.change(readout.benchmark)}</span>
        </span>
      </div>
      <div ref={host} className="equity-chart compare-host" tabIndex={0} onKeyDown={onKeyDown} role="img" aria-label={`Returns compared: ${description}`} aria-describedby={hintId} />
      <p id={hintId} className="visually-hidden">
        {KEY_HINT}
      </p>
    </div>
  );
}

function MetricTable({ entries }: { entries: Compared[] }) {
  const reference = entries[0];
  const paramKeys = [...new Set(entries.flatMap((entry) => Object.keys(entry.run.config.params)))];
  const differing = paramKeys.filter((key) => new Set(entries.map((entry) => JSON.stringify(entry.run.config.params[key]))).size > 1);
  const cell = (key: MetricKey, entry: Compared) => {
    const value = entry.metrics?.[key];
    return value === null || value === undefined ? undefined : key === "max_drawdown" ? -value : value;
  };
  const rows = [
    { id: "excess", name: "CAGR vs SPY", title: "The run's CAGR minus total-return SPY's over the same sessions; — where either is undefined, such as under a year of sessions.", better: "high" as Better, values: entries.map(excessCagr), render: fmt.change },
    ...ROWS.map(({ key, better }) => ({
      id: key,
      name: METRICS[key].name,
      title: METRICS[key].title,
      better,
      values: entries.map((entry) => cell(key, entry)),
      render: (value: number | undefined) => (key === "max_drawdown" ? fmt.change(value) : METRICS[key].render(value)),
    })),
  ];

  return (
    <section className="panel" aria-label="Metrics compared">
      <header className="panel-head">
        <h2>Metrics</h2>
        <span className="muted">Differences are against {reference ? label(reference.run) : "the first run"}; the best value in a row is marked</span>
      </header>
      <div className="table-wrap">
        <table className="table fin compare-table">
          <thead>
            <tr>
              <th scope="col">
                <span className="visually-hidden">Metric</span>
              </th>
              {entries.map((entry, index) => (
                <th key={entry.run.id} scope="col" className="r" style={chip(index)}>
                  <span className="compare-swatch" aria-hidden="true" />
                  <a href={href({ page: "bot", name: entry.run.bot, run: entry.run.id })}>{label(entry.run)}</a>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => {
              const top = best(row.values, row.better);
              return (
                <tr key={row.id}>
                  <th scope="row">
                    {row.name}
                    <Info label={row.name}>{row.title}</Info>
                  </th>
                  {row.values.map((value, index) => {
                    const diff = index > 0 ? delta(value, row.values[0], row.id === "excess" ? "excess" : (row.id as MetricKey)) : undefined;
                    const signed = row.id === "excess" || (row.id !== "excess" && METRICS[row.id as MetricKey].tone !== undefined);
                    return (
                      <td key={entries[index]?.run.id} className={top !== undefined && value === top ? "r compare-best" : "r"}>
                        <span className={`num ${signed ? (tone(value) ?? "") : ""}`}>{value === undefined ? ABSENT : row.render(value)}</span>
                        {diff && <span className="compare-delta num">{diff}</span>}
                      </td>
                    );
                  })}
                </tr>
              );
            })}
            {differing.map((key) => (
              <tr key={`param-${key}`} className="compare-param">
                <th scope="row" className="mono">
                  {key}
                </th>
                {entries.map((entry) => (
                  <td key={entry.run.id} className="r mono">
                    {JSON.stringify(entry.run.config.params[key] ?? null)}
                  </td>
                ))}
              </tr>
            ))}
            <tr className="compare-param">
              <th scope="row">Badges</th>
              {entries.map((entry) => (
                <td key={entry.run.id} className="r badges">
                  <RunBadges run={entry.run} />
                </td>
              ))}
            </tr>
          </tbody>
        </table>
      </div>
      {differing.length === 0 && entries.length > 1 && <p className="muted compare-note">Every run has the same params: {paramsText(reference?.run.config.params ?? {}) || "none"}.</p>}
    </section>
  );
}
