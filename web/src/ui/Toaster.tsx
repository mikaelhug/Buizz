import { useEffect, useState } from "react";

export type ToastKind = "error" | "success" | "info";
type Toast = { id: number; message: string; kind: ToastKind };

const TOAST_EVENT = "buizz-toast";
const TOAST_MS = 6000;

let nextId = 0;

export function notify(message: string, kind: ToastKind = "error"): void {
  window.dispatchEvent(new CustomEvent(TOAST_EVENT, { detail: { message, kind } }));
}

export function Toaster() {
  const [toasts, setToasts] = useState<Toast[]>([]);

  useEffect(() => {
    const timers = new Set<ReturnType<typeof setTimeout>>();
    const onToast = (event: Event) => {
      const { message, kind } = (event as CustomEvent<Omit<Toast, "id">>).detail;
      const id = ++nextId;
      setToasts((current) => [...current, { id, message, kind }]);
      const timer = setTimeout(() => {
        timers.delete(timer);
        setToasts((current) => current.filter((toast) => toast.id !== id));
      }, TOAST_MS);
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
        <button
          key={toast.id}
          type="button"
          className={`toast toast-${toast.kind}`}
          onClick={() => setToasts((current) => current.filter((other) => other.id !== toast.id))}
        >
          {toast.message}
        </button>
      ))}
    </div>
  );
}
