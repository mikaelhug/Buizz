import type { Schemas } from "../api/client";
import { label } from "../lib/format";
import { Explain } from "./Popover";

type Status = Schemas["BotStatus"] | Schemas["RunStatus"] | Schemas["OrderStatus"] | Schemas["JobOutcome"];

const STATUS_TEXT: Partial<Record<Status, { text: string; about: string }>> = {
  stopped: {
    text: "drawdown stop",
    about: "The latest run ended at its drawdown stop: equity fell by the limit from its peak, every position was closed and the run ended.",
  },
  error: { text: "last run failed", about: "The bot's latest run failed. Open the run and its Logs tab for the bot's output." },
  idle: { text: "idle", about: "Nothing is running: no live paper run and no backtest in progress." },
  backtesting: { text: "backtesting", about: "A backtest of this bot is queued or running; results appear as soon as it completes." },
};

export function statusText(status: Status): string {
  return STATUS_TEXT[status]?.text ?? label(status);
}

export function StatusPill({ status }: { status: Status }) {
  const spec = STATUS_TEXT[status];
  const className = `pill pill-${status}`;
  return spec ? (
    <Explain label={spec.text} className={`${className} explained`} trigger={spec.text}>
      {spec.about}
    </Explain>
  ) : (
    <span className={className}>{label(status)}</span>
  );
}

const BADGES: Record<Schemas["RunBadge"], { text: string; about: string }> = {
  survivorship_biased: {
    text: "Survivorship bias",
    about: "The universe was picked from today's listings (a fixed symbol list, or a top dollar volume ranking while the store holds no delisted names), so names delisted before the run are missing and results look better than they would have.",
  },
  holdout: { text: "Holdout period", about: "Ran on the locked last 12 months, kept aside as an out-of-sample test." },
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
      {run.origin === "screening" && (
        <Explain label="Automatic screening" className="badge badge-button" trigger="Automatic screening">
          {run.mode === "live"
            ? "This variant went live with every screened variant at once, from its backtest that passes the screening gate."
            : "The spine queued this backtest of the bot's default params on the 10-year preset by itself, at start-up, because the bot's current code had none queued, running or completed."}
        </Explain>
      )}
      {run.badges.map((badge) => (
        <RunBadge key={badge} badge={badge} universe={run.config.universe} />
      ))}
    </>
  );
}

export function UnscreenedBadge({ reason }: { reason: string }) {
  return (
    <Explain label="Not screened" className="badge badge-button badge-stale" trigger="Not screened">
      Automatic screening queued no 10-year backtest of this bot: {reason}.
    </Explain>
  );
}

export function HoldoutBadge({ runs }: { runs: number }) {
  return (
    <Explain label="Holdout runs" className="badge badge-button badge-holdout" trigger={`Holdout ×${runs}`}>
      This bot has run {runs === 1 ? "once" : `${runs} times`} on the locked last 12 months. Every look at the holdout makes it less of an honest out-of-sample test.
    </Explain>
  );
}
