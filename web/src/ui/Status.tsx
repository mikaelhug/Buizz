import type { Schemas } from "../api/client";
import { label } from "../lib/format";

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

const BADGES: Record<Schemas["RunBadge"], { text: string; short: string; title: string }> = {
  survivorship_biased: {
    text: "Survivorship-biased",
    short: "Biased",
    title: "Universe built from today's listings; names delisted before the run are missing",
  },
  holdout: { text: "Holdout", short: "Holdout", title: "Ran on the locked last 12 months" },
  data_warnings: { text: "Data warnings", short: "Data", title: "The run saw data warnings; see its events" },
  drawdown_stop: {
    text: "Drawdown stop",
    short: "DD stop",
    title: "Equity fell by the max drawdown limit from its peak; every position was closed and the run ended",
  },
  thin_index_history: {
    text: "Thin index history",
    short: "Thin index",
    title: "The run starts before the index change log is complete enough to rebuild membership reliably",
  },
};

export function RunBadges({ badges, short = false }: { badges: Schemas["RunBadge"][]; short?: boolean }) {
  return (
    <>
      {badges.map((badge) => (
        <span key={badge} className={`badge badge-${badge} help`} title={short ? `${BADGES[badge].text}: ${BADGES[badge].title}` : BADGES[badge].title}>
          {short ? BADGES[badge].short : BADGES[badge].text}
        </span>
      ))}
    </>
  );
}
