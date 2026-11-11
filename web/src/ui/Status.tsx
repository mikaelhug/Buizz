import type { Schemas } from "../api/client";
import { label } from "../lib/format";

export function StatusPill({ status }: { status: Schemas["BotStatus"] | Schemas["RunStatus"] | Schemas["OrderStatus"] | Schemas["JobOutcome"] }) {
  return <span className={`pill pill-${status}`}>{label(status)}</span>;
}

const BADGES: Record<Schemas["RunBadge"], { text: string; title: string }> = {
  survivorship_biased: {
    text: "Survivorship-biased",
    title: "Universe built from today's listings; names delisted before the run are missing",
  },
  holdout: { text: "Holdout", title: "Ran on the locked last 12 months" },
  data_warnings: { text: "Data warnings", title: "The run saw data warnings; see its events" },
  thin_index_history: {
    text: "Thin index history",
    title: "The run starts before the index change log is complete enough to rebuild membership reliably",
  },
};

const DRAWDOWN_STOP = "stopped by drawdown limit";

export function RunBadges({ run }: { run: Schemas["RunSummary"] }) {
  return (
    <>
      {run.status === "failed" && run.message === DRAWDOWN_STOP && (
        <span className="badge badge-drawdown_stop help" title="Equity fell by the max drawdown limit from its peak; every position was closed and the run ended">
          Drawdown stop
        </span>
      )}
      {run.badges.map((badge) => (
        <span key={badge} className={`badge badge-${badge} help`} title={BADGES[badge].title}>
          {BADGES[badge].text}
        </span>
      ))}
    </>
  );
}
