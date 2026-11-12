import { type ReactNode, useState } from "react";
import type { Schemas } from "../api/client";
import { label } from "../lib/format";
import { Popover } from "./Popover";

type Status = Schemas["BotStatus"] | Schemas["RunStatus"] | Schemas["OrderStatus"] | Schemas["JobOutcome"];

const STATUS_ABOUT: Partial<Record<Status, string>> = {
  stopped: "The latest run ended at its drawdown stop: equity fell by the limit from its peak, every position was closed and the run ended.",
};

function Explained({ className, text, title, children }: { className: string; text: ReactNode; title: string; children: ReactNode }) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  return (
    <>
      <button
        type="button"
        className={`${className} explained`}
        aria-haspopup="dialog"
        aria-expanded={anchor !== null}
        onClick={(event) => setAnchor(anchor ? null : event.currentTarget)}
      >
        {text}
      </button>
      {anchor && (
        <Popover anchor={anchor} label={title} onClose={() => setAnchor(null)}>
          <div className="popover-title">{title}</div>
          {children}
        </Popover>
      )}
    </>
  );
}

export function StatusPill({ status }: { status: Status }) {
  const about = STATUS_ABOUT[status];
  const className = `pill pill-${status}`;
  return about ? (
    <Explained className={className} text={label(status)} title={label(status)}>
      {about}
    </Explained>
  ) : (
    <span className={className}>{label(status)}</span>
  );
}

const BADGES: Record<Schemas["RunBadge"], { text: string; short: string; about: string }> = {
  survivorship_biased: {
    text: "Survivorship-biased",
    short: "Biased",
    about: "The universe was built from today's listings, so names delisted before the run are missing and results look better than they would have.",
  },
  holdout: { text: "Holdout", short: "Holdout", about: "Ran on the locked last 12 months, kept aside as an out-of-sample test." },
  data_warnings: { text: "Data warnings", short: "Data", about: "The run saw data warnings; the Positions & Events tab lists them." },
  drawdown_stop: {
    text: "Drawdown stop",
    short: "DD stop",
    about: "Equity fell by the max drawdown limit from its peak; every position was closed and the run ended.",
  },
  thin_index_history: {
    text: "Thin index history",
    short: "Thin index",
    about: "The run starts before the index change log is complete enough to rebuild membership reliably.",
  },
};

export function RunBadges({ badges, short = false }: { badges: Schemas["RunBadge"][]; short?: boolean }) {
  return (
    <>
      {badges.map((badge) => (
        <Explained key={badge} className={`badge badge-${badge}`} text={short ? BADGES[badge].short : BADGES[badge].text} title={BADGES[badge].text}>
          {BADGES[badge].about}
        </Explained>
      ))}
    </>
  );
}

export function HoldoutBadge({ runs }: { runs: number }) {
  return (
    <Explained className="badge badge-holdout" text={`Holdout ×${runs}`} title="Holdout runs">
      This bot has run {runs === 1 ? "once" : `${runs} times`} on the locked last 12 months. Every look at the holdout makes it less of an honest out-of-sample test.
    </Explained>
  );
}
