import type { Schemas } from "../api/client";
import { label } from "../lib/format";
import { Explain } from "./Popover";

type Status = Schemas["BotStatus"] | Schemas["RunStatus"] | Schemas["OrderStatus"] | Schemas["JobOutcome"];

const STATUS_TITLES: Partial<Record<Status, string>> = {
  stopped: "The latest run ended at its drawdown stop",
};

export function StatusPill({ status }: { status: Status }) {
  const title = STATUS_TITLES[status];
  return (
    <span className={`pill pill-${status}${title ? " help" : ""}`} title={title}>
      {label(status)}
    </span>
  );
}

const BADGES: Record<Schemas["RunBadge"], { text: string; title: string }> = {
  survivorship_biased: {
    text: "Survivorship-biased",
    title: "Universe built from today's listings; names delisted before the run are missing",
  },
  holdout: { text: "Holdout", title: "Ran on the locked last 12 months" },
  data_warnings: { text: "Data warnings", title: "The run saw data warnings; see its events" },
  drawdown_stop: {
    text: "Drawdown stop",
    title: "Equity fell by the max drawdown limit from its peak; every position was closed and the run ended",
  },
  thin_index_history: {
    text: "Thin index history",
    title: "The run starts before the index change log is complete enough to rebuild membership reliably",
  },
};

function RunBadge({ badge }: { badge: Schemas["RunBadge"] }) {
  const { text, title } = BADGES[badge];
  return (
    <Explain label={text} className={`badge badge-button badge-${badge}`} trigger={text}>
      {title}
    </Explain>
  );
}

export function RunBadges({ badges }: { badges: Schemas["RunBadge"][] }) {
  return (
    <>
      {badges.map((badge) => (
        <RunBadge key={badge} badge={badge} />
      ))}
    </>
  );
}
