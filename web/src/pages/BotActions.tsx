import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { confirmDialog } from "../ui/Dialog";
import { notify } from "../ui/Toaster";

type Props = {
  bot: Schemas["BotSummary"];
  onChanged: (run: Schemas["RunSummary"]) => void;
};

export function BotActions({ bot, onChanged }: Props) {
  const [busy, setBusy] = useState(false);
  const live = bot.status === "live";
  const liveRun = bot.live_run;

  const act = async (pending: () => Promise<Schemas["RunSummary"]>, done: string) => {
    setBusy(true);
    try {
      const run = await pending();
      notify(`${bot.name} ${done} · run #${run.id}`, "success");
      onChanged(run);
    } catch (error) {
      notify(`${bot.name}: ${errorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const toggle = () => {
    const params = { params: { path: { name: bot.name } } };
    return act(
      () => must(live ? api.POST("/api/bots/{name}/pause", params) : api.POST("/api/bots/{name}/live", params)),
      live ? "paused" : "live",
    );
  };

  const stop = async (id: number) => {
    const confirmed = await confirmDialog({
      title: `Stop live run #${id}?`,
      message: `${bot.name} cancels its working orders and the run ends`,
      confirmLabel: "Stop run",
      danger: true,
    });
    if (confirmed) {
      await act(() => must(api.POST("/api/runs/{id}/cancel", { params: { path: { id } } })), "stopped");
    }
  };

  return (
    <span className="bot-actions">
      {liveRun !== null && liveRun !== undefined && (
        <button type="button" className="btn btn-sm btn-ghost" disabled={busy} onClick={() => void stop(liveRun)} title="End the live run for good">
          Stop
        </button>
      )}
      <button
        type="button"
        className={`btn btn-sm ${live ? "btn-pause" : "btn-live"}`}
        disabled={busy}
        onClick={() => void toggle()}
        title={live ? "Cancel working orders and stop the bot; equity keeps marking" : bot.status === "paused" ? "Resume the paused live run" : "Start a live paper run"}
      >
        {live ? "Pause" : "Live"}
      </button>
    </span>
  );
}
