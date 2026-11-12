import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { confirmDialog } from "../ui/Dialog";
import { Icon } from "../ui/Icon";
import { Info } from "../ui/Popover";
import { notify } from "../ui/Toaster";

type Run = Schemas["RunSummary"];

type Props = {
  name: string;
  live: Run | undefined;
  onChanged: (run: Run) => void;
};

export function isRunning(run: Run | undefined): boolean {
  return run !== undefined && run.status !== "paused" && run.desired !== "paused";
}

export function BotActions({ name, live: liveRun, onChanged }: Props) {
  const [busy, setBusy] = useState(false);
  const live = isRunning(liveRun);

  const act = async (pending: () => Promise<Run>, done: string) => {
    setBusy(true);
    try {
      const run = await pending();
      notify(`${name} ${done} · run #${run.id}`, "success");
      onChanged(run);
    } catch (error) {
      notify(`${name}: ${errorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  const toggle = () => {
    const params = { params: { path: { name } } };
    return act(
      () => must(live ? api.POST("/api/bots/{name}/pause", params) : api.POST("/api/bots/{name}/live", params)),
      live ? "paused" : "live",
    );
  };

  const stop = async (id: number) => {
    const confirmed = await confirmDialog({
      title: `Stop live run #${id}?`,
      message: `${name} cancels its working orders and the run ends`,
      confirmLabel: "Stop run",
      danger: true,
    });
    if (confirmed) {
      await act(() => must(api.POST("/api/runs/{id}/cancel", { params: { path: { id } } })), "stopped");
    }
  };

  return (
    <span className="bot-actions">
      {liveRun && (
        <button type="button" className="btn btn-sm btn-ghost" disabled={busy} onClick={() => void stop(liveRun.id)}>
          Stop
        </button>
      )}
      <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void toggle()}>
        <Icon name={live ? "pause" : "play"} />
        {live ? "Pause" : "Live"}
      </button>
      <Info label="Live, Pause and Stop">
        Live starts a live paper run, or resumes a paused one. Pause cancels working orders and stops the bot while its equity keeps marking. Stop ends the
        live run for good.
      </Info>
    </span>
  );
}
