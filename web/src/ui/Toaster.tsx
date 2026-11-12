import { useEffect, useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { type SpineEvent, useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { href } from "../lib/route";

export type ToastKind = "error" | "warn" | "success" | "info";
type ToastLink = { href: string; text: string };
type Toast = { id: number; message: string; kind: ToastKind; link?: ToastLink };

const TOAST_EVENT = "buizz-toast";
const TOAST_MS = 6000;
const LINKED_TOAST_MS = 10_000;

let nextId = 0;

export function notify(message: string, kind: ToastKind = "error", link?: ToastLink): void {
  window.dispatchEvent(new CustomEvent(TOAST_EVENT, { detail: { message, kind, link } }));
}

type RunEnded = Extract<SpineEvent, { type: "run_status" }> & { status: "completed" | "failed" };

const ended = (event: SpineEvent): event is RunEnded => event.type === "run_status" && (event.status === "completed" || event.status === "failed");

function runNotice(event: RunEnded, run: Schemas["RunSummary"] | undefined): { message: string; kind: ToastKind } {
  const name = `${event.bot} ${run?.mode === "live" ? "live run" : "backtest"} #${event.run_id}`;
  if (event.status === "completed") {
    const result = run?.metrics ? ` · ${fmt.change(run.metrics.total_return)}` : "";
    return { message: `${name} finished${result}`, kind: "success" };
  }
  if (run?.badges.includes("drawdown_stop")) {
    return { message: `${name} stopped at its drawdown limit`, kind: "warn" };
  }
  return { message: `${name} failed${run?.message ? `: ${run.message}` : ""}`, kind: "error" };
}

function useRunNotices() {
  useSpineEvents((event) => {
    if (!ended(event)) {
      return;
    }
    const link = { href: href({ page: "bot", name: event.bot, run: event.run_id }), text: "View run" };
    must(api.GET("/api/runs/{id}", { params: { path: { id: event.run_id } } })).then(
      (run) => runNotice(event, run),
      () => runNotice(event, undefined),
    ).then(({ message, kind }) => notify(message, kind, link));
  });
}

export function Toaster() {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const dismiss = (id: number) => setToasts((current) => current.filter((toast) => toast.id !== id));

  useRunNotices();

  useEffect(() => {
    const timers = new Set<ReturnType<typeof setTimeout>>();
    const onToast = (event: Event) => {
      const toast = { ...(event as CustomEvent<Omit<Toast, "id">>).detail, id: ++nextId };
      setToasts((current) => [...current, toast]);
      const timer = setTimeout(
        () => {
          timers.delete(timer);
          setToasts((current) => current.filter((other) => other.id !== toast.id));
        },
        toast.link ? LINKED_TOAST_MS : TOAST_MS,
      );
      timers.add(timer);
    };
    window.addEventListener(TOAST_EVENT, onToast);
    return () => {
      window.removeEventListener(TOAST_EVENT, onToast);
      timers.forEach(clearTimeout);
    };
  }, []);

  return (
    <div className="toaster" role="status" aria-live="polite">
      {toasts.map((toast) => (
        <div key={toast.id} className={`toast toast-${toast.kind}`}>
          <span className="toast-message">{toast.message}</span>
          {toast.link && (
            <a className="toast-link" href={toast.link.href} onClick={() => dismiss(toast.id)}>
              {toast.link.text}
            </a>
          )}
          <button type="button" className="toast-close" aria-label="Dismiss" onClick={() => dismiss(toast.id)}>
            ×
          </button>
        </div>
      ))}
    </div>
  );
}
