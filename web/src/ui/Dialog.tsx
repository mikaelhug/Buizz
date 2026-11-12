import { type ReactNode, useEffect, useRef, useState } from "react";

type ModalProps = {
  title: string;
  confirmLabel: string;
  danger?: boolean;
  busy?: boolean;
  onConfirm: () => void;
  onCancel: () => void;
  children?: ReactNode;
};

export function Modal({ title, confirmLabel, danger = false, busy = false, onConfirm, onCancel, children }: ModalProps) {
  const ref = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    const dialog = ref.current;
    dialog?.showModal();
    return () => dialog?.close();
  }, []);

  return (
    <dialog
      ref={ref}
      className="dialog"
      aria-label={title}
      onCancel={(event) => {
        event.preventDefault();
        onCancel();
      }}
      onClick={(event) => event.target === event.currentTarget && onCancel()}
    >
      <form
        className="dialog-body"
        onSubmit={(event) => {
          event.preventDefault();
          onConfirm();
        }}
      >
        <h2 className="dialog-title">{title}</h2>
        {children}
        <div className="dialog-actions">
          <button type="button" className="btn" onClick={onCancel}>
            Cancel
          </button>
          <button type="submit" className={`btn ${danger ? "btn-danger" : "btn-primary"}`} disabled={busy} autoFocus>
            {confirmLabel}
          </button>
        </div>
      </form>
    </dialog>
  );
}

type ConfirmRequest = {
  title: string;
  message: string | undefined;
  confirmLabel: string;
  danger: boolean;
  resolve: (confirmed: boolean) => void;
};

const CONFIRM_EVENT = "buizz-confirm";

export function confirmDialog(options: {
  title: string;
  message?: string;
  confirmLabel?: string;
  danger?: boolean;
}): Promise<boolean> {
  return new Promise((resolve) => {
    const request: ConfirmRequest = {
      title: options.title,
      message: options.message,
      confirmLabel: options.confirmLabel ?? "Confirm",
      danger: options.danger ?? false,
      resolve,
    };
    window.dispatchEvent(new CustomEvent(CONFIRM_EVENT, { detail: request }));
  });
}

export function DialogHost() {
  const [request, setRequest] = useState<ConfirmRequest | null>(null);

  useEffect(() => {
    const onRequest = (event: Event) => setRequest((event as CustomEvent<ConfirmRequest>).detail);
    window.addEventListener(CONFIRM_EVENT, onRequest);
    return () => window.removeEventListener(CONFIRM_EVENT, onRequest);
  }, []);

  if (!request) {
    return null;
  }
  const settle = (confirmed: boolean) => {
    request.resolve(confirmed);
    setRequest(null);
  };
  return (
    <Modal
      title={request.title}
      confirmLabel={request.confirmLabel}
      danger={request.danger}
      onConfirm={() => settle(true)}
      onCancel={() => settle(false)}
    >
      {request.message && <p className="dialog-message">{request.message}</p>}
    </Modal>
  );
}
