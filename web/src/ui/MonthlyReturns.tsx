import { useMemo } from "react";
import type { Schemas } from "../api/client";
import { exchangeWallSeconds, fmt, tone } from "../lib/format";

type Point = Schemas["SparkPoint"];
type Year = { year: number; months: (number | undefined)[]; total: number; benchmark: number | undefined };

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
const FULL_TINT_RETURN = 0.1;

function monthEnds(points: Point[]): Map<number, number> {
  const ends = new Map<number, number>();
  for (const point of points) {
    const wall = new Date(exchangeWallSeconds(point.at) * 1000);
    ends.set(wall.getUTCFullYear() * 12 + wall.getUTCMonth(), Number(point.equity));
  }
  return ends;
}

function yearEnds(ends: Map<number, number>): Map<number, number> {
  return new Map([...ends].map(([month, equity]) => [Math.floor(month / 12), equity]));
}

function change(end: number, base: number): number {
  return end / base - 1;
}

function calendar(points: Point[], benchmark: Point[], startCash: number): Year[] {
  const ends = monthEnds(points);
  const through = points.at(-1)?.at;
  const benchmarkYears = yearEnds(monthEnds(through ? benchmark.filter((point) => Date.parse(point.at) <= Date.parse(through)) : []));
  const years = new Map<number, Year>();
  let base = startCash;
  let yearBase = startCash;
  let benchmarkBase = startCash;
  for (const [month, equity] of ends) {
    const year = Math.floor(month / 12);
    let row = years.get(year);
    if (!row) {
      row = { year, months: Array.from({ length: 12 }, () => undefined), total: 0, benchmark: undefined };
      years.set(year, row);
    }
    row.months[month % 12] = change(equity, base);
    row.total = change(equity, yearBase);
    base = equity;
    if (month % 12 === 11) {
      yearBase = equity;
    }
  }
  for (const row of years.values()) {
    const benchmarkEnd = benchmarkYears.get(row.year);
    if (benchmarkEnd !== undefined) {
      row.benchmark = change(benchmarkEnd, benchmarkBase);
      benchmarkBase = benchmarkEnd;
    }
  }
  return [...years.values()];
}

function Cell({ value, className }: { value: number | undefined; className?: string }) {
  if (value === undefined) {
    return <td className={className} />;
  }
  const strength = Math.round(Math.min(Math.abs(value) / FULL_TINT_RETURN, 1) * 40);
  const color = value >= 0 ? "var(--up)" : "var(--down)";
  return (
    <td className={`r num ${className ?? ""}`} style={{ background: `color-mix(in srgb, ${color} ${strength}%, transparent)` }}>
      {fmt.change(value)}
    </td>
  );
}

type Props = { points: Point[]; benchmark: Point[]; benchmarkSymbol: string; startCash: string };

export function MonthlyReturns({ points, benchmark, benchmarkSymbol, startCash }: Props) {
  const years = useMemo(() => calendar(points, benchmark, Number(startCash)), [points, benchmark, startCash]);
  if (years.length === 0) {
    return null;
  }
  return (
    <section className="panel">
      <h2>Monthly returns</h2>
      <div className="table-wrap">
        <table className="table monthly">
          <thead>
            <tr>
              <th />
              {MONTHS.map((month) => (
                <th key={month} className="r">
                  {month}
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
                {row.months.map((value, month) => (
                  <Cell key={MONTHS[month]} value={value} />
                ))}
                <Cell value={row.total} className="year" />
                <td className={`r num ${tone(row.benchmark) ?? ""}`}>{fmt.change(row.benchmark)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
