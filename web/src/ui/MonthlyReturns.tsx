import { useState } from "react";
import type { Schemas } from "../api/client";
import { exchangeWallSeconds, fmt, tone } from "../lib/format";
import type { MonthRange } from "./Metrics";
import { Info, Popover } from "./Popover";

type Point = Schemas["SparkPoint"];
type Month = { strategy: number; benchmark: number | undefined };
type Year = { year: number; months: Map<number, Month>; total: number; benchmark: number | undefined };

export type Calendar = { years: Year[]; months: number[]; range: MonthRange };

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const MONTH_NAMES = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const SCALE_STEPS = [0.02, 0.05, 0.1, 0.2, 0.5, 1];
const MAX_TINT = 55;

function monthEnds(points: Point[]): Map<number, number> {
  const ends = new Map<number, number>();
  for (const point of points) {
    const wall = new Date(exchangeWallSeconds(point.at) * 1000);
    ends.set(wall.getUTCFullYear() * 12 + wall.getUTCMonth(), Number(point.equity));
  }
  return ends;
}

function chained(ends: Map<number, number>, start: number): Map<number, number> {
  const changes = new Map<number, number>();
  let base = start;
  for (const [month, equity] of ends) {
    changes.set(month, equity / base - 1);
    base = equity;
  }
  return changes;
}

function yearly(ends: Map<number, number>, start: number): Map<number, number> {
  const changes = new Map<number, number>();
  let base = start;
  for (const [month, equity] of ends) {
    const year = Math.floor(month / 12);
    changes.set(year, equity / base - 1);
    if (month % 12 === 11) {
      base = equity;
    }
  }
  return changes;
}

export function monthlyCalendar(points: Point[], benchmark: Point[], startCash: string): Calendar {
  const start = Number(startCash);
  const through = points.at(-1)?.at;
  const ends = monthEnds(points);
  const benchmarkEnds = monthEnds(through ? benchmark.filter((point) => Date.parse(point.at) <= Date.parse(through)) : []);
  const strategy = chained(ends, start);
  const reference = chained(benchmarkEnds, start);
  const strategyYears = yearly(ends, start);
  const benchmarkYears = yearly(benchmarkEnds, start);
  const years = new Map<number, Year>();
  for (const [month, value] of strategy) {
    const year = Math.floor(month / 12);
    const row = years.get(year) ?? { year, months: new Map(), total: strategyYears.get(year) ?? 0, benchmark: benchmarkYears.get(year) };
    row.months.set(month % 12, { strategy: value, benchmark: reference.get(month) });
    years.set(year, row);
  }
  const values = [...strategy.values()];
  return {
    years: [...years.values()],
    months: [...new Set([...strategy.keys()].map((month) => month % 12))].toSorted((a, b) => a - b),
    range: values.length > 0 ? { best: Math.max(...values), worst: Math.min(...values) } : undefined,
  };
}

function scaleFor(years: Year[]): number {
  const largest = Math.max(...years.flatMap((row) => [...row.months.values()].map((month) => Math.abs(month.strategy))));
  return SCALE_STEPS.find((step) => largest <= step) ?? 1;
}

function tint(value: number, scale: number): string {
  const strength = Math.round(Math.min(Math.abs(value) / scale, 1) * MAX_TINT);
  return `color-mix(in srgb, ${value >= 0 ? "var(--up)" : "var(--down)"} ${strength}%, transparent)`;
}

function MonthCell({ year, month, value, scale, symbol }: { year: number; month: number; value: Month; scale: number; symbol: string }) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  const title = `${MONTH_NAMES[month]} ${year}`;
  const difference = value.benchmark === undefined ? undefined : value.strategy - value.benchmark;
  return (
    <td className="r num month" style={{ background: tint(value.strategy, scale) }}>
      <button
        type="button"
        aria-haspopup="dialog"
        aria-expanded={anchor !== null}
        aria-label={`${title}: ${fmt.change(value.strategy)}, ${symbol} ${fmt.change(value.benchmark)}`}
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
      >
        {fmt.change(value.strategy)}
      </button>
      {anchor && (
        <Popover anchor={anchor} label={title} width={220} onClose={() => setAnchor(null)}>
          <div className="popover-title">{title}</div>
          <dl className="key-values">
            <div>
              <dt>Bot</dt>
              <dd className="num">{fmt.change(value.strategy)}</dd>
            </div>
            <div>
              <dt>{symbol}</dt>
              <dd className="num">{fmt.change(value.benchmark)}</dd>
            </div>
            <div>
              <dt>Difference</dt>
              <dd className="num">{fmt.change(difference)}</dd>
            </div>
          </dl>
        </Popover>
      )}
    </td>
  );
}

function Legend({ scale }: { scale: number }) {
  const bound = fmt.percent(scale).replace(/\.00%$/, "%");
  return (
    <div className="monthly-legend">
      <span className="num">−{bound}</span>
      <span className="monthly-scale" aria-hidden="true" />
      <span className="num">+{bound}</span>
      <span className="visually-hidden">Cell colour runs from red for a loss of {bound} or more, through clear at zero, to green for a gain of {bound} or more.</span>
    </div>
  );
}

type Props = { calendar: Calendar; benchmarkSymbol: string };

export function MonthlyReturns({ calendar, benchmarkSymbol }: Props) {
  const { years, months } = calendar;
  if (years.length === 0) {
    return null;
  }
  const scale = scaleFor(years);
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Monthly returns</h2>
        <Info label="Monthly returns">
          Change in equity from one month-end close to the next; the first month starts from the start cash and the last runs to the latest mark. Open a cell for {benchmarkSymbol} over the same month.
        </Info>
        <Legend scale={scale} />
      </header>
      <div className="table-wrap">
        <table className="table monthly">
          <thead>
            <tr>
              <th />
              {months.map((month) => (
                <th key={month} className="r">
                  {MONTHS[month]}
                </th>
              ))}
              <th className="r">Year</th>
              <th className="r">{benchmarkSymbol}</th>
            </tr>
          </thead>
          <tbody>
            {years.map((row) => (
              <tr key={row.year}>
                <th scope="row">{row.year}</th>
                {months.map((month) => {
                  const value = row.months.get(month);
                  return value ? <MonthCell key={month} year={row.year} month={month} value={value} scale={scale} symbol={benchmarkSymbol} /> : <td key={month} />;
                })}
                <td className={`r num year ${tone(row.total) ?? ""}`}>{fmt.change(row.total)}</td>
                <td className="r num muted">{fmt.change(row.benchmark)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
