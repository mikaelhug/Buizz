import type { Schemas } from "../api/client";

const WIDTH = 120;
const HEIGHT = 32;

export function Sparkline({ points }: { points: Schemas["SparkPoint"][] }) {
  const values = points.map((point) => Number(point.equity));
  const first = values[0];
  const last = values.at(-1);
  if (values.length < 2 || first === undefined || last === undefined) {
    return <svg className="sparkline" viewBox={`0 0 ${WIDTH} ${HEIGHT}`} aria-hidden="true" />;
  }
  const low = Math.min(...values);
  const span = Math.max(...values) - low || 1;
  const step = WIDTH / (values.length - 1);
  const path = values
    .map((value, index) => `${(index * step).toFixed(1)},${(HEIGHT - 2 - ((value - low) / span) * (HEIGHT - 4)).toFixed(1)}`)
    .join(" ");
  const baseline = HEIGHT - 2 - ((first - low) / span) * (HEIGHT - 4);
  return (
    <svg className={`sparkline ${last >= first ? "up" : "down"}`} viewBox={`0 0 ${WIDTH} ${HEIGHT}`} preserveAspectRatio="none" aria-hidden="true">
      <line x1="0" x2={WIDTH} y1={baseline} y2={baseline} className="sparkline-base" />
      <polyline points={path} />
    </svg>
  );
}
