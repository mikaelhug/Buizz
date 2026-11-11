import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { notify } from "../ui/Toaster";

type Props = {
  bot: Schemas["BotSummary"];
  onChanged: (run: Schemas["RunSummary"]) => void;
};

export function BotActions({ bot, onChanged }: Props) {
  const [busy, setBusy] = useState(false);
  const live = bot.status === "live";

  const act = async () => {
    setBusy(true);
    try {
      const params = { params: { path: { name: bot.name } } };
      const run = await must(live ? api.POST("/api/bots/{name}/pause", params) : api.POST("/api/bots/{name}/live", params));
      notify(`${bot.name} ${live ? "paused" : "live"} · run #${run.id}`, "success");
      onChanged(run);
    } catch (error) {
      notify(`${bot.name}: ${errorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <button
      type="button"
      className={`btn btn-sm ${live ? "btn-pause" : "btn-live"}`}
      disabled={busy}
      onClick={() => void act()}
      title={live ? "Cancel working orders and stop the bot; equity keeps marking" : bot.status === "paused" ? "Resume the paused live run" : "Start a live paper run"}
    >
      {live ? "Pause" : "Live"}
    </button>
  );
}
