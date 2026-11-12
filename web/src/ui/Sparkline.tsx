import type { Schemas } from "../api/client";
import { fmt } from "../lib/format";

const WIDTH = 100;
const HEIGHT = 28;
const PAD = 2;

export type Mark = { at: string; equity: string };

type Props = { curve: Schemas["LivePoint"][]; mark: Mark | undefined; benchmark: string };

export function Sparkline({ curve, mark, benchmark }: Props) {
  const first = curve[0];
  const strategy = [...curve.map((point) => Number(point.equity)), ...(mark ? [Number(mark.equity)] : [])];
  const index = curve.map((point) => Number(point.benchmark));
  if (!first || strategy.length < 2) {
    return <div className="sparkline sparkline-empty" aria-hidden="true" />;
  }
  const values = [...strategy, ...index];
  const low = Math.min(...values);
  const span = Math.max(...values) - low || 1;
  const step = WIDTH / (strategy.length - 1);
  const y = (value: number) => HEIGHT - PAD - ((value - low) / span) * (HEIGHT - 2 * PAD);
  const line = (series: number[]) => series.map((value, at) => `${(at * step).toFixed(2)},${y(value).toFixed(2)}`).join(" ");
  const start = Number(first.equity);
  const change = (value: number | undefined) => fmt.change(value === undefined ? undefined : value / start - 1);
  const label = `Since ${fmt.date(first.at)} the run is ${change(strategy.at(-1))} and ${benchmark} ${change(index.at(-1))}`;
  return (
    <figure className="sparkline" role="img" aria-label={label}>
      <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" aria-hidden="true">
        <line x1="0" x2={WIDTH} y1={y(start)} y2={y(start)} className="sparkline-base" />
        <polyline points={line(index)} className="sparkline-benchmark" />
        <polyline points={line(strategy)} className="sparkline-run" />
      </svg>
    </figure>
  );
}
