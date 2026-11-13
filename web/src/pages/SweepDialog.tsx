import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { Modal } from "../ui/Dialog";
import { Info } from "../ui/Popover";
import { PresetDates } from "../ui/PresetDates";
import { notify } from "../ui/Toaster";

export type Preset = Exclude<Schemas["PeriodPreset"], "holdout">;

export const SWEEP_PRESETS: readonly Preset[] = ["1y", "3y", "5y", "10y"];
const MAX_VARIANTS = 256;

type Props = {
  name: string;
  defaults: Record<string, unknown>;
  preset: Preset;
  onStarted: (started: Schemas["SweepStarted"], preset: Preset) => void;
  onClose: () => void;
};

function shown(value: unknown): string {
  return typeof value === "string" ? value : JSON.stringify(value);
}

function parsed(text: string): unknown {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

export function sweepValues(text: string): unknown[] {
  return text
    .split(",")
    .map((piece) => piece.trim())
    .filter((piece) => piece !== "")
    .map(parsed);
}

export function isPreset(value: string | undefined): value is Preset {
  return SWEEP_PRESETS.some((preset) => preset === value);
}

export function SweepDialog({ name, defaults, preset: initial, onStarted, onClose }: Props) {
  const [preset, setPreset] = useState<Preset>(initial);
  const [texts, setTexts] = useState<Record<string, string>>(() => Object.fromEntries(Object.entries(defaults).map(([key, value]) => [key, shown(value)])));
  const [busy, setBusy] = useState(false);
  const grid = Object.fromEntries(
    Object.entries(texts)
      .filter(([key, text]) => text.trim() !== shown(defaults[key]))
      .map(([key, text]) => [key, sweepValues(text)]),
  );
  const variants = Object.values(grid).reduce((count, values) => count * values.length, 1);
  const tooMany = variants > MAX_VARIANTS;
  const empty = Object.values(grid).some((values) => values.length === 0);

  const start = async () => {
    setBusy(true);
    try {
      const started = await must(api.POST("/api/bots/{name}/sweeps", { params: { path: { name } }, body: { preset, grid } }));
      notify(`${started.queued.length} backtests queued, ${started.skipped} already run`, "success");
      onStarted(started, preset);
    } catch (error) {
      notify(errorMessage(error));
      setBusy(false);
    }
  };

  return (
    <Modal title={`Sweep ${name}`} confirmLabel={`Queue ${variants} ${variants === 1 ? "backtest" : "backtests"}`} busy={busy || tooMany || empty} onConfirm={() => void start()} onCancel={onClose}>
      <div className="segmented" role="radiogroup" aria-label="Period">
        {SWEEP_PRESETS.map((option) => (
          <button key={option} type="button" role="radio" aria-checked={preset === option} className={preset === option ? "active" : undefined} onClick={() => setPreset(option)}>
            {option}
          </button>
        ))}
      </div>
      <PresetDates preset={preset} className="dialog-message muted" />
      {Object.keys(defaults).length === 0 ? (
        <p className="dialog-message">{name} has no params; the sweep is one backtest of its defaults.</p>
      ) : (
        <fieldset className="sweep-grid" aria-label="Param values">
          {Object.entries(texts).map(([key, text]) => (
            <div key={key} className="field">
              <span className="field-label">
                <label htmlFor={`sweep-${key}`} className="mono">
                  {key}
                </label>
                <span className="muted">default {shown(defaults[key])}</span>
              </span>
              <input
                id={`sweep-${key}`}
                className="input mono"
                type="text"
                value={text}
                onChange={(event) => {
                  const value = event.target.value;
                  setTexts((current) => ({ ...current, [key]: value }));
                }}
              />
            </div>
          ))}
        </fieldset>
      )}
      <p className={tooMany ? "dialog-message sweep-count down" : "dialog-message sweep-count"} role="status">
        {variants} {variants === 1 ? "variant" : "variants"}
        {tooMany && `; a sweep runs at most ${MAX_VARIANTS}`}
        <Info label="Sweeps">
          Comma-separated values per param; every combination is backtested on the {preset} period before the holdout with default costs and one shared seed. Variants
          already run on the current code are skipped, and every variant counts as a trial.
        </Info>
      </p>
    </Modal>
  );
}
