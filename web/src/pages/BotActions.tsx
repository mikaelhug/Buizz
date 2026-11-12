import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { Modal } from "../ui/Dialog";
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

export function goLive(name: string, fromRun?: number): Promise<Run> {
  return must(api.POST("/api/bots/{name}/live", { params: { path: { name } }, body: fromRun === undefined ? {} : { from_run: fromRun } }));
}

export function pauseRun(id: number): Promise<Run> {
  return must(api.POST("/api/runs/{id}/pause", { params: { path: { id } } }));
}

export function resumeRun(id: number): Promise<Run> {
  return must(api.POST("/api/runs/{id}/resume", { params: { path: { id } } }));
}

export function undoPause(runs: Run[]): { text: string; run: () => void } {
  return {
    text: "Undo",
    run: () =>
      void Promise.all(runs.map((run) => resumeRun(run.id))).then(
        () => notify(runs.length === 1 ? `${runs[0]?.bot} live again` : `${runs.length} runs live again`, "success"),
        (error: unknown) => notify(errorMessage(error)),
      ),
  };
}

export function useLiveControls(name: string, liveRun: Run | undefined, onChanged: (run: Run) => void) {
  const [busy, setBusy] = useState(false);
  const [stopping, setStopping] = useState<Run>();
  const live = isRunning(liveRun);

  const act = async (pending: () => Promise<Run>, done: string) => {
    setBusy(true);
    try {
      const run = await pending();
      notify(`${name} ${done} · run #${run.id}`, "success", done === "paused" ? undoPause([run]) : undefined);
      onChanged(run);
      return true;
    } catch (error) {
      notify(`${name}: ${errorMessage(error)}`);
      return false;
    } finally {
      setBusy(false);
    }
  };

  const toggle = () => {
    if (!liveRun) {
      return act(() => goLive(name), "live");
    }
    return act(() => (live ? pauseRun(liveRun.id) : resumeRun(liveRun.id)), live ? "paused" : "live");
  };

  const dialog = stopping && (
    <StopDialog
      name={name}
      run={stopping}
      busy={busy}
      onCancel={() => setStopping(undefined)}
      onStop={(reason) =>
        void act(() => must(api.POST("/api/runs/{id}/cancel", { params: { path: { id: stopping.id } }, body: reason ? { reason } : {} })), reason ? "retired" : "stopped").then(
          (stopped) => stopped && setStopping(undefined),
        )
      }
    />
  );

  return {
    busy,
    live,
    toggleText: live ? "Pause" : liveRun ? "Live" : "Go live",
    toggle: () => void toggle(),
    stop: liveRun ? () => setStopping(liveRun) : undefined,
    dialog,
  };
}

export function BotActions({ name, live: liveRun, onChanged }: Props) {
  const controls = useLiveControls(name, liveRun, onChanged);
  return (
    <span className="bot-actions">
      {controls.stop && (
        <button type="button" className="btn btn-sm btn-ghost" disabled={controls.busy} onClick={controls.stop}>
          Stop
        </button>
      )}
      <button type="button" className="btn btn-sm" disabled={controls.busy} onClick={controls.toggle}>
        <Icon name={controls.live ? "pause" : "play"} />
        {controls.toggleText}
      </button>
      <Info label="Live, Pause and Stop">
        Go live starts a live paper run with the bot's default params; a variant goes live from its backtest under Research. Live resumes a paused run.
        Pause cancels working orders and stops the bot while its equity keeps marking. Stop ends the live run for good; with a reason it is retired.
      </Info>
      {controls.dialog}
    </span>
  );
}

function StopDialog({ name, run, busy, onStop, onCancel }: { name: string; run: Run; busy: boolean; onStop: (reason: string) => void; onCancel: () => void }) {
  const [reason, setReason] = useState("");
  return (
    <Modal title={`Stop live run #${run.id}?`} confirmLabel="Stop run" danger busy={busy} onConfirm={() => onStop(reason.trim())} onCancel={onCancel}>
      <p className="dialog-message">{name} cancels its working orders and the run ends.</p>
      <label className="stop-reason">
        Reason (retires the run)
        <input className="input" type="text" placeholder="Optional, e.g. outside its backtest envelope" value={reason} onChange={(event) => setReason(event.target.value)} />
      </label>
    </Modal>
  );
}
