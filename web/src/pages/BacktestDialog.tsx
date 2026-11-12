import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { PRESETS, type Preset } from "../lib/route";
import { Modal } from "../ui/Dialog";
import { Info } from "../ui/Popover";
import { notify } from "../ui/Toaster";

type Props = {
  bot: Schemas["BotSummary"];
  risk: Schemas["RiskLimits"];
  onStarted: (run: Schemas["RunSummary"]) => void;
  onClose: () => void;
};

type Choice = Exclude<Preset, "holdout"> | "custom";

type Limit = keyof Schemas["RiskLimits"];

type Percents = Record<Limit, string>;

const CHOICES: readonly Choice[] = [...PRESETS.filter((preset) => preset !== "holdout"), "custom"];

const LIMITS: readonly { key: Limit; label: string; title: string }[] = [
  { key: "max_position_weight", label: "Max position %", title: "Reject orders that grow one position past this share of equity" },
  { key: "max_gross_exposure", label: "Max gross %", title: "Reject orders that grow the book past this share of equity; above 100 allows leverage" },
  { key: "max_drawdown", label: "Drawdown stop %", title: "Close every position and end the run when equity falls this far from its peak" },
];

function scaled(value: string, factor: number): string {
  return String(Number((Number(value) * factor).toPrecision(12)));
}

function percents(risk: Schemas["RiskLimits"]): Percents {
  const shown = (fraction: string | null | undefined) => (fraction ? scaled(fraction, 100) : "");
  return {
    max_position_weight: shown(risk.max_position_weight),
    max_gross_exposure: shown(risk.max_gross_exposure),
    max_drawdown: shown(risk.max_drawdown),
  };
}

function fractions(edited: Percents): Schemas["RiskLimits"] {
  const fraction = (percent: string) => (percent.trim() === "" ? null : scaled(percent, 0.01));

  return {
    max_position_weight: fraction(edited.max_position_weight),
    max_gross_exposure: fraction(edited.max_gross_exposure),
    max_drawdown: fraction(edited.max_drawdown),
  };
}

function request(choice: Choice, period: Schemas["Period"], holdout: boolean): Schemas["StartBacktest"] {
  if (choice === "custom") {
    return { period, allow_holdout: holdout };
  }
  return holdout ? { preset: "holdout", allow_holdout: true } : { preset: choice };
}

export function BacktestDialog({ bot, risk, onStarted, onClose }: Props) {
  const [choice, setChoice] = useState<Choice>("5y");
  const [period, setPeriod] = useState<Schemas["Period"]>({ from: "", to: "" });
  const [holdout, setHoldout] = useState(false);
  const [limits, setLimits] = useState<Percents>(() => percents(risk));
  const [busy, setBusy] = useState(false);
  const custom = choice === "custom";
  const defaults = percents(risk);
  const overridden = LIMITS.some(({ key }) => limits[key].trim() !== defaults[key]);

  const start = async () => {
    setBusy(true);
    try {
      const body = request(choice, period, holdout);
      const run = await must(
        api.POST("/api/bots/{name}/backtests", {
          params: { path: { name: bot.name } },
          body: overridden ? { ...body, risk: fractions(limits) } : body,
        }),
      );
      notify(`Backtest #${run.id} ${run.status}`, "success");
      onStarted(run);
    } catch (error) {
      notify(errorMessage(error));
      setBusy(false);
    }
  };

  return (
    <Modal
      title={`Backtest ${bot.name}`}
      confirmLabel={holdout && !custom ? "Run on holdout" : "Run"}
      danger={holdout}
      busy={busy}
      onConfirm={() => void start()}
      onCancel={onClose}
    >
      <div className="segmented" role="radiogroup" aria-label="Period">
        {CHOICES.map((option) => {
          const active = choice === option && (custom || !holdout);
          return (
            <button
              key={option}
              type="button"
              role="radio"
              aria-checked={active}
              className={active ? "active" : undefined}
              disabled={holdout && !custom && option !== "custom"}
              onClick={() => setChoice(option)}
            >
              {option}
            </button>
          );
        })}
      </div>
      {custom && (
        <div className="period-range">
          <label>
            From
            <input
              className="input"
              type="date"
              required
              value={period.from}
              max={period.to || undefined}
              onChange={(event) => {
                const from = event.target.value;
                setPeriod((current) => ({ ...current, from }));
              }}
            />
          </label>
          <label>
            To
            <input
              className="input"
              type="date"
              required
              value={period.to}
              min={period.from || undefined}
              onChange={(event) => {
                const to = event.target.value;
                setPeriod((current) => ({ ...current, to }));
              }}
            />
          </label>
        </div>
      )}
      <span className="toggle-row">
        <label className="toggle">
          <input type="checkbox" checked={holdout} onChange={(event) => setHoldout(event.target.checked)} />
          Holdout
        </label>
        <Info label="Holdout">
          {custom ? "Allow the period to reach into the locked last 12 months" : "Run on the locked last 12 months instead"}; counted per bot ({bot.holdout_runs} so far).
        </Info>
      </span>
      <fieldset className="risk-limits" aria-label="Risk limits">
        {LIMITS.map(({ key, label, title }) => (
          <div key={key} className="field">
            <span className="field-label">
              <label htmlFor={`limit-${key}`}>{label}</label>
              <Info label={label}>
                {title}. Empty for no limit; the bot's own is {defaults[key] || "none"}.
              </Info>
            </span>
            <input
              id={`limit-${key}`}
              className="input"
              type="number"
              min="0"
              step="any"
              placeholder="none"
              value={limits[key]}
              onChange={(event) => {
                const value = event.target.value;
                setLimits((current) => ({ ...current, [key]: value }));
              }}
            />
          </div>
        ))}
      </fieldset>
    </Modal>
  );
}
