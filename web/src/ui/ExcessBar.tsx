import { tone } from "../lib/format";

export const EXCESS_SCALE_MIN = 0.05;

export function excessScale(values: (number | undefined)[]): number {
  return Math.max(EXCESS_SCALE_MIN, ...values.map((value) => Math.abs(value ?? 0)));
}

export function ExcessBar({ value, scale }: { value: number | undefined; scale: number }) {
  const share = value === undefined ? 0 : Math.min(1, Math.abs(value) / scale) * 50;
  return (
    <span className="excess-bar" aria-hidden="true">
      {value !== undefined && <span className={`excess-fill ${tone(value) ?? ""}`} style={value >= 0 ? { left: "50%", width: `${share}%` } : { right: "50%", width: `${share}%` }} />}
    </span>
  );
}
