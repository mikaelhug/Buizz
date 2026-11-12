import { type ReactNode, useEffect, useEffectEvent, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Icon } from "./Icon";

const GAP = 6;
const MARGIN = 8;
const FOCUSABLE = '[aria-checked="true"], button, a[href], input, select, textarea, [tabindex]:not([tabindex="-1"])';

type PopoverProps = {
  anchor: HTMLElement;
  label: string;
  onClose: () => void;
  width?: number;
  children: ReactNode;
};

export function Popover({ anchor, label, onClose, width = 280, children }: PopoverProps) {
  const ref = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState<{ top: number; left: number }>();

  const close = useEffectEvent((restoreFocus: boolean) => {
    onClose();
    if (restoreFocus) {
      anchor.focus();
    }
  });

  const place = useEffectEvent(() => {
    const popover = ref.current;
    const rect = anchor.getBoundingClientRect();
    if (!popover || rect.bottom < 0 || rect.top > window.innerHeight) {
      close(false);
      return;
    }
    const left = Math.max(MARGIN, Math.min(rect.left, window.innerWidth - popover.offsetWidth - MARGIN));
    const below = rect.bottom + GAP;
    const above = rect.top - GAP - popover.offsetHeight;
    const flip = below + popover.offsetHeight > window.innerHeight - MARGIN && above > MARGIN;
    setPosition((current) => {
      const top = flip ? above : below;
      return current?.top === top && current.left === left ? current : { top, left };
    });
  });

  useLayoutEffect(() => {
    place();
  });

  useEffect(() => {
    const popover = ref.current;
    (popover?.querySelector<HTMLElement>(FOCUSABLE) ?? popover)?.focus();
    const inside = (target: EventTarget | null) => target instanceof Node && (popover?.contains(target) || anchor.contains(target));
    const onPointerDown = (event: PointerEvent) => !inside(event.target) && close(false);
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        close(true);
      }
    };
    const onFocusOut = (event: FocusEvent) => event.relatedTarget && !inside(event.relatedTarget) && close(false);
    const onMove = () => place();
    document.addEventListener("pointerdown", onPointerDown);
    popover?.addEventListener("keydown", onKeyDown);
    popover?.addEventListener("focusout", onFocusOut);
    window.addEventListener("scroll", onMove, true);
    window.addEventListener("resize", onMove);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      popover?.removeEventListener("keydown", onKeyDown);
      popover?.removeEventListener("focusout", onFocusOut);
      window.removeEventListener("scroll", onMove, true);
      window.removeEventListener("resize", onMove);
    };
  }, [anchor]);

  return createPortal(
    <div
      ref={ref}
      className="popover"
      role="dialog"
      aria-label={label}
      tabIndex={-1}
      style={{ width, top: position?.top ?? 0, left: position?.left ?? 0 }}
    >
      {children}
    </div>,
    anchor.closest("dialog") ?? document.body,
  );
}

type ExplainProps = { label: string; className: string; ariaLabel?: string; trigger: ReactNode; children: ReactNode };

export function Explain({ label, className, ariaLabel, trigger, children }: ExplainProps) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  return (
    <>
      <button
        type="button"
        className={className}
        aria-label={ariaLabel}
        aria-haspopup="dialog"
        aria-expanded={anchor !== null}
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
      >
        {trigger}
      </button>
      {anchor && (
        <Popover anchor={anchor} label={label} onClose={() => setAnchor(null)}>
          <div className="popover-title">{label}</div>
          {children}
        </Popover>
      )}
    </>
  );
}

export function Info({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Explain label={label} className="info" ariaLabel={`About ${label}`} trigger={<Icon name="info" />}>
      {children}
    </Explain>
  );
}
