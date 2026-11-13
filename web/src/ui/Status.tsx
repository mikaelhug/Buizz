import type { Schemas } from "../api/client";
import { label } from "../lib/format";
import { Explain } from "./Popover";

type Status = Schemas["BotStatus"] | Schemas["RunStatus"] | Schemas["OrderStatus"] | Schemas["JobOutcome"];

const STATUS_ABOUT: Partial<Record<Status, string>> = {
  stopped: "The latest run ended at its drawdown stop: equity fell by the limit from its peak, every position was closed and the run ended.",
};

export function StatusPill({ status }: { status: Status }) {
  const about = STATUS_ABOUT[status];
  const className = `pill pill-${status}`;
  return about ? (
    <Explain label={label(status)} className={`${className} explained`} trigger={label(status)}>
      {about}
    </Explain>
  ) : (
    <span className={className}>{label(status)}</span>
  );
}

const BADGES: Record<Schemas["RunBadge"], { text: string; about: string }> = {
  survivorship_biased: {
    text: "Survivorship-biased",
    about: "The universe was picked from today's listings (a fixed symbol list, or a top dollar volume ranking while the store holds no delisted names), so names delisted before the run are missing and results look better than they would have.",
  },
  holdout: { text: "Holdout", about: "Ran on the locked last 12 months, kept aside as an out-of-sample test." },
  data_warnings: { text: "Data warnings", about: "The run saw data warnings; the Positions & events tab lists them." },
  drawdown_stop: {
    text: "Drawdown stop",
    about: "Equity fell by the max drawdown limit from its peak; every position was closed and the run ended.",
  },
  thin_index_history: {
    text: "Thin index history",
    about: "The run starts before the index change log is complete enough to rebuild membership reliably.",
  },
  minor_data_gaps: {
    text: "Minor data gaps",
    about:
      "A few index members have no prices on some sessions of their membership, at most 0.5% of the universe's member-sessions, and the bot never ordered or held them then, so the screening gate lets it pass. The Positions & events tab lists each gap.",
  },
};

const INDEX_SURVIVORSHIP =
  "Some index members have no prices for part of their membership, so the bot could not trade them then and results may look better than they would have. The Positions & events tab lists each gap.";

function RunBadge({ badge, universe }: { badge: Schemas["RunBadge"]; universe: Schemas["UniverseSpec"] }) {
  const { text, about } = BADGES[badge];
  const indexed = badge === "survivorship_biased" && "index" in universe;
  return (
    <Explain label={text} className={`badge badge-button badge-${badge}`} trigger={text}>
      {indexed ? INDEX_SURVIVORSHIP : about}
    </Explain>
  );
}

export function RunBadges({ run }: { run: Schemas["RunSummary"] }) {
  return (
    <>
      {run.badges.map((badge) => (
        <RunBadge key={badge} badge={badge} universe={run.config.universe} />
      ))}
    </>
  );
}

export function HoldoutBadge({ runs }: { runs: number }) {
  return (
    <Explain label="Holdout runs" className="badge badge-button badge-holdout" trigger={`Holdout ×${runs}`}>
      This bot has run {runs === 1 ? "once" : `${runs} times`} on the locked last 12 months. Every look at the holdout makes it less of an honest out-of-sample test.
    </Explain>
  );
}
