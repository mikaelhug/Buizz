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

const PERIODS = PRESETS.filter((preset) => preset !== "holdout");

export function BacktestDialog({ bot, onStarted, onClose }: Props) {
  const [preset, setPreset] = useState<Preset>("5y");
  const [holdout, setHoldout] = useState(false);
  const [busy, setBusy] = useState(false);

  const start = async () => {
    setBusy(true);
    try {
      const run = await must(
        api.POST("/api/bots/{name}/backtests", {
          params: { path: { name: bot.name } },
          body: holdout ? { preset: "holdout", allow_holdout: true } : { preset },
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
      confirmLabel={holdout ? "Run on holdout" : "Run"}
      danger={holdout}
      busy={busy}
      onConfirm={() => void start()}
      onCancel={onClose}
    >
      <div className="segmented" role="radiogroup" aria-label="Period">
        {PERIODS.map((option) => (
          <button
            key={option}
            type="button"
            role="radio"
            aria-checked={!holdout && preset === option}
            className={!holdout && preset === option ? "active" : undefined}
            disabled={holdout}
            onClick={() => setPreset(option)}
          >
            {option}
          </button>
        ))}
      </div>
      <label className="toggle help" title={`Run on the locked last 12 months instead; counted per bot (${bot.holdout_runs} so far)`}>
        <input type="checkbox" checked={holdout} onChange={(event) => setHoldout(event.target.checked)} />
        Holdout
      </label>
    </Modal>
  );
}
