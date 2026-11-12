import { PriceScaleMode } from "lightweight-charts";
import type { Preset } from "./model";

export type ScaleMode = "price" | "percent" | "log";

const SCALES: { mode: ScaleMode; label: string; title: string }[] = [
  { mode: "price", label: "$", title: "Dollar scale" },
  { mode: "percent", label: "%", title: "Percent change from the start of the visible range" },
  { mode: "log", label: "Log", title: "Logarithmic scale" },
];

export const SCALE_MODES: Record<ScaleMode, PriceScaleMode> = {
  price: PriceScaleMode.Normal,
  percent: PriceScaleMode.Percentage,
  log: PriceScaleMode.Logarithmic,
};

type RangeProps = { presets: Preset[]; active: Preset | null; resetTitle: string; onPick: (preset: Preset) => void };

export function RangePresets({ presets, active, resetTitle, onPick }: RangeProps) {
  return (
    <>
      {presets.length > 1 && (
        <div className="segmented segmented-sm" role="group" aria-label="Visible range">
          {presets.map((option) => (
            <button key={option} type="button" aria-pressed={active === option} className={active === option ? "active" : undefined} onClick={() => onPick(option)}>
              {option}
            </button>
          ))}
        </div>
      )}
      {active === null && (
        <button type="button" className="btn btn-ghost btn-xs" onClick={() => onPick("All")} title={resetTitle}>
          Reset
        </button>
      )}
    </>
  );
}

export function ScaleSwitch({ mode, onChange }: { mode: ScaleMode; onChange: (mode: ScaleMode) => void }) {
  return (
    <div className="segmented segmented-sm" role="group" aria-label="Price scale">
      {SCALES.map((option) => (
        <button
          key={option.mode}
          type="button"
          title={option.title}
          aria-pressed={mode === option.mode}
          className={mode === option.mode ? "active" : undefined}
          onClick={() => onChange(option.mode)}
        >
          {option.label}
        </button>
      ))}
    </div>
  );
}
