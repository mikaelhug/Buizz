import { useId } from "react";
import type { Schemas } from "../api/client";
import { fmt } from "../lib/format";

const WIDTH = 100;
const HEIGHT = 40;
const PAD = 3;

const day = (at: string) => at.slice(0, 10);

export function Sparkline({ points }: { points: Schemas["SparkPoint"][] }) {
  const gradient = useId();
  const values = points.map((point) => Number(point.equity));
  const first = points[0];
  const last = points.at(-1);
  if (values.length < 2 || !first || !last) {
    return <div className="sparkline sparkline-empty" aria-hidden="true" />;
  }
  const low = Math.min(...values);
  const high = Math.max(...values);
  const span = high - low || 1;
  const y = (value: number) => HEIGHT - PAD - ((value - low) / span) * (HEIGHT - 2 * PAD);
  const step = WIDTH / (values.length - 1);
  const line = values.map((value, index) => `${(index * step).toFixed(2)},${y(value).toFixed(2)}`).join(" ");
  const start = values[0] ?? low;
  const end = values.at(-1) ?? low;
  const baseline = y(start);
  const split = `${((baseline / HEIGHT) * 100).toFixed(2)}%`;
  const label = `Equity from ${fmt.money(first.equity)} on ${day(first.at)} to ${fmt.money(last.equity)} on ${day(last.at)}, low ${fmt.money(low)}, high ${fmt.money(high)}`;
  return (
    <figure className="sparkline" role="img" aria-label={label}>
      <div className="sparkline-plot">
        <svg viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" aria-hidden="true">
          <defs>
            <linearGradient id={gradient} x1="0" x2="0" y1="0" y2={HEIGHT} gradientUnits="userSpaceOnUse">
              <stop offset={split} className="sparkline-above" />
              <stop offset={split} className="sparkline-below" />
            </linearGradient>
          </defs>
          <line x1="0" x2={WIDTH} y1={baseline} y2={baseline} className="sparkline-base" />
          <polygon points={`0,${baseline} ${line} ${WIDTH},${baseline}`} fill={`url(#${gradient})`} className="sparkline-area" />
          <polyline points={line} stroke={`url(#${gradient})`} />
        </svg>
        <span className={`sparkline-dot ${end >= start ? "up" : "down"}`} style={{ top: `${(y(end) / HEIGHT) * 100}%` }} />
      </div>
      <figcaption className="sparkline-axis" aria-hidden="true">
        <span>{day(first.at)}</span>
        <span>{day(last.at)}</span>
      </figcaption>
    </figure>
  );
}
