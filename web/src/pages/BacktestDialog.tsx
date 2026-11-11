import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { PRESETS, type Preset } from "../lib/route";
import { Modal } from "../ui/Dialog";
import { notify } from "../ui/Toaster";

type Props = {
  bot: Schemas["BotSummary"];
  onStarted: (run: Schemas["RunSummary"]) => void;
  onClose: () => void;
};

type Choice = Exclude<Preset, "holdout"> | "custom";

const CHOICES: readonly Choice[] = [...PRESETS.filter((preset) => preset !== "holdout"), "custom"];

function request(choice: Choice, period: Schemas["Period"], holdout: boolean): Schemas["StartBacktest"] {
  if (choice === "custom") {
    return { period, allow_holdout: holdout };
  }
  return holdout ? { preset: "holdout", allow_holdout: true } : { preset: choice };
}

export function BacktestDialog({ bot, onStarted, onClose }: Props) {
  const [choice, setChoice] = useState<Choice>("5y");
  const [period, setPeriod] = useState<Schemas["Period"]>({ from: "", to: "" });
  const [holdout, setHoldout] = useState(false);
  const [busy, setBusy] = useState(false);
  const custom = choice === "custom";

  const start = async () => {
    setBusy(true);
    try {
      const run = await must(
        api.POST("/api/bots/{name}/backtests", {
          params: { path: { name: bot.name } },
          body: request(choice, period, holdout),
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
      <label
        className="toggle help"
        title={
          custom
            ? `Allow the period to reach into the locked last 12 months; counted per bot (${bot.holdout_runs} so far)`
            : `Run on the locked last 12 months instead; counted per bot (${bot.holdout_runs} so far)`
        }
      >
        <input type="checkbox" checked={holdout} onChange={(event) => setHoldout(event.target.checked)} />
        Holdout
      </label>
    </Modal>
  );
}
