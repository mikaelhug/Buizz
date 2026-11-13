import type { ReactNode } from "react";
import type { Schemas } from "../api/client";
import { fmt, tone } from "../lib/format";
import { href } from "../lib/route";
import { Explain, Info } from "./Popover";
import { StatusPill } from "./Status";
import { SortHeader, type SortColumns, type Sorting, type SortValue } from "./Table";

export type Live = Schemas["LiveSummary"];
type Stats = Schemas["LiveStats"];

export type Column = {
  label: string;
  about?: string;
  numeric?: boolean;
  ascending?: boolean;
  value: (live: Live) => SortValue;
  cell: (live: Live) => ReactNode;
};

export type LiveKey =
  | "bot"
  | "sessions"
  | "excess_cagr"
  | "total_return"
  | "excess_return"
  | "max_drawdown"
  | "confidence"
  | "verdict"
  | "sessions_needed"
  | "status";

const stat = <T,>(live: Live, pick: (stats: Stats) => T) => (live.run.live_stats ? pick(live.run.live_stats) : undefined);

function Signed({ value }: { value: number | null | undefined }) {
  return <span className={`num ${tone(value) ?? ""}`}>{fmt.change(value)}</span>;
}

export const YEAR_SESSIONS = 252;

function PlusMinus({ value }: { value: number | null | undefined }) {
  return value === null || value === undefined ? null : (
    <span className="se">
      <span aria-hidden="true">±</span>
      <span className="visually-hidden">plus or minus</span>
      {fmt.percent(value)}
    </span>
  );
}

function Excess({ live }: { live: Live }) {
  const stats = live.run.live_stats;
  return (
    <>
      <Signed value={stats?.excess_return} />
      <PlusMinus value={stats?.excess_error} />
    </>
  );
}

export function Annualized({ value, sessions, error }: { value: number | null | undefined; sessions: number; error?: number | null | undefined }) {
  const short = value !== null && value !== undefined && sessions < YEAR_SESSIONS;
  return (
    <span className={short ? "annualized-short" : undefined} title={short ? `annualized from ${fmt.integer(sessions)} sessions` : undefined}>
      <Signed value={value} />
      <PlusMinus value={error} />
      {short && <span className="annualized-note">from {fmt.integer(sessions)} sessions</span>}
    </span>
  );
}

const ANNUALIZED = `Annualized over the sessions since the run went live. Under ${YEAR_SESSIONS} sessions it is greyed and says how many sessions it comes from: a few weeks annualized is not a track record`;

function annualizedColumn(label: string, about: string, pick: (stats: Stats) => number, error?: (stats: Stats) => number | null | undefined): Column {
  return {
    label,
    about: `${about}. ${ANNUALIZED}`,
    numeric: true,
    value: (live) => stat(live, pick),
    cell: (live) => (
      <Annualized value={stat(live, pick)} sessions={stat(live, (stats) => stats.sessions) ?? 0} error={error && stat(live, error)} />
    ),
  };
}

export function retired(live: Live): boolean {
  return ["completed", "failed", "cancelled"].includes(live.run.status);
}

export function LiveStatus({ live }: { live: Live }) {
  return retired(live) ? <span className="pill pill-retired">retired</span> : <StatusPill status={live.run.status} />;
}

type Breach = Schemas["EnvelopeBreach"];

const VOLATILITY_LIMIT = 1.5;
const DECAY = "Live results usually fall short of their backtest: 26–58% decay against the backtest is normal, so only leaving the band counts, never merely trailing the backtest.";

const BREACH_NAMES: Record<Breach, string> = { excess: "excess return", drawdown: "drawdown", volatility: "volatility" };

export const JUDGED_FROM_SESSION = 20;

const VERDICT_ABOUT: Record<Schemas["LiveVerdict"], string> = {
  incubating: `Too early to judge. Live results are compared with the backtest it came from once it has ${JUDGED_FROM_SESSION} sessions, and it joins the League ranking then.`,
  tracking: "On track: inside the range its backtest predicts on every test, but not yet proven to beat SPY.",
  no_backtest:
    "No completed backtest of this code and params outside the holdout, so there is no range to compare with: it can never be off track. Its live results alone can still prove it.",
  falsified: "Off track: outside the range its backtest predicts, so the backtest no longer describes it. Nothing stops it automatically; retiring it is your call.",
  proven: "Beats SPY: the probabilistic Sharpe of its daily excess return over SPY reached 1 − 0.05 ÷ live runs started, and it is inside its backtest's range.",
};

const VERDICT_LABEL: Record<Exclude<Schemas["LiveVerdict"], "incubating">, string> = {
  tracking: "On track",
  no_backtest: "No backtest to compare",
  falsified: "Off track",
  proven: "Beats SPY",
};

const closeTime = new Intl.DateTimeFormat("en-GB", { weekday: "short", day: "numeric", month: "short", hour: "2-digit", minute: "2-digit" });
const closeDay = new Intl.DateTimeFormat("en-GB", { weekday: "short", hour: "2-digit", minute: "2-digit" });

export function firstClose(live: Live, long = false): string | undefined {
  const session = live.first_session;
  return session ? (long ? closeTime : closeDay).format(new Date(session.close)) : undefined;
}

export function sessionsOf(live: Live): number {
  return live.run.live_stats?.sessions ?? 0;
}

export function verdictLabel(live: Live): string {
  const verdict = live.run.live_stats?.verdict ?? "incubating";
  if (verdict !== "incubating") {
    return VERDICT_LABEL[verdict];
  }
  const sessions = sessionsOf(live);
  const starts = firstClose(live);
  return sessions === 0 ? (starts ? `Starts ${starts}` : "Starting") : `Day ${sessions} of ${JUDGED_FROM_SESSION}`;
}

export function verdictAbout(live: Live): string {
  const verdict = live.run.live_stats?.verdict ?? "incubating";
  const starts = firstClose(live, true);
  if (verdict === "incubating" && sessionsOf(live) === 0) {
    return `Live paper since ${live.run.config.start}. It trades for the first time at the close of ${starts ?? "the next session"}; after every close its equity is marked against SPY. ${VERDICT_ABOUT.incubating}`;
  }
  return VERDICT_ABOUT[verdict];
}

function EnvelopeTests({ live, stats }: { live: Live; stats: Stats }) {
  const envelope = stats.envelope;
  if (!envelope) {
    return null;
  }
  const backtestVolatility = envelope.volatility;
  const volatilityLimit = backtestVolatility === null || backtestVolatility === undefined ? undefined : backtestVolatility * VOLATILITY_LIMIT;
  const tests: { breach: Breach; name: string; value: string; band: string }[] = [
    { breach: "excess", name: "Excess return", value: fmt.change(stats.excess_return), band: `at least ${fmt.change(envelope.excess_floor)}, the 5th percentile` },
    { breach: "drawdown", name: "Max drawdown", value: fmt.percent(stats.max_drawdown), band: `at most ${fmt.percent(envelope.drawdown_ceiling)}, the 95th percentile` },
    {
      breach: "volatility",
      name: "Volatility",
      value: fmt.percent(stats.volatility),
      band: `at most ${fmt.percent(volatilityLimit)}, ${VOLATILITY_LIMIT}× the backtest's ${fmt.percent(backtestVolatility)}`,
    },
  ];
  return (
    <>
      <p>
        Envelope from backtest <a href={href({ page: "bot", name: live.run.bot, run: envelope.source })}>#{envelope.source}</a>, over its {fmt.integer(envelope.windows)} rolling windows of{" "}
        {fmt.integer(stats.sessions)} sessions (as old as this run) across {fmt.integer(envelope.source_sessions)} backtest sessions.
        {envelope.windows === 0 && " The run is now older than its backtest, so only the volatility test applies."}
      </p>
      <table className="envelope-tests">
        <tbody>
          {tests.map((test) => {
            const failed = envelope.breaches.includes(test.breach);
            return (
              <tr key={test.breach} className={failed ? "down" : undefined}>
                <th scope="row">{test.name}</th>
                <td className="num">{test.value}</td>
                <td>
                  {failed ? "outside: " : "inside: "}
                  {test.band}
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </>
  );
}

export function Verdict({ live }: { live: Live }) {
  const stats = live.run.live_stats;
  const verdict = stats?.verdict ?? "incubating";
  const breaches = stats?.envelope?.breaches ?? [];
  const text = verdictLabel(live);
  return (
    <Explain label={text} className={`pill pill-${verdict} explained`} trigger={text}>
      <p>
        {verdictAbout(live)}
        {verdict === "falsified" && ` Failed: ${breaches.map((breach) => BREACH_NAMES[breach]).join(", ")}.`}
      </p>
      {stats && stats.sessions > 0 && <EnvelopeTests live={live} stats={stats} />}
      {verdict !== "incubating" && <p>{DECAY}</p>}
    </Explain>
  );
}

const negative = (value: number | null | undefined) => (value === null || value === undefined ? value : -value);

export const LIVE_COLUMNS: Record<LiveKey, Column> = {
  bot: {
    label: "Bot",
    value: (live) => live.run.bot,
    cell: (live) => <a href={href({ page: "bot", name: live.run.bot, run: live.run.id })}>{live.run.bot}</a>,
  },

  sessions: {
    label: "Sessions",
    about: "Session closes since the run went live; paused sessions count, since the run keeps its positions marked while paused",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.sessions),
    cell: (live) => <span className="num">{fmt.integer(stat(live, (stats) => stats.sessions))}</span>,
  },
  excess_cagr: annualizedColumn(
    "CAGR vs SPY",
    "The run's CAGR minus its benchmark's CAGR over exactly the same sessions. The ± figure is one standard error of the annualized mean daily excess return",
    (stats) => stats.excess_cagr,
    (stats) => stats.excess_cagr_error,
  ),
  total_return: {
    label: "Return",
    about: "Change in equity since the run went live, after every cost, through the latest session close",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.total_return),
    cell: (live) => <Signed value={stat(live, (stats) => stats.total_return)} />,
  },

  excess_return: {
    label: "Excess",
    about:
      "Return minus the benchmark's over the same window. The ± figure is one standard error: the spread of daily excess returns times the square root of the sessions, so an excess within it of zero is not yet distinguishable from luck",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.excess_return),
    cell: (live) => <Excess live={live} />,
  },
  max_drawdown: {
    label: "Max DD",
    about: "Largest fall in session-close equity from its running peak since the run went live",
    numeric: true,
    ascending: true,
    value: (live) => stat(live, (stats) => stats.max_drawdown),
    cell: (live) => <Signed value={negative(stat(live, (stats) => stats.max_drawdown))} />,
  },


  confidence: {
    label: "Confidence",
    about:
      "Probabilistic Sharpe ratio of daily excess returns over the benchmark: the probability that the true Sharpe of the excess is above zero, allowing for the track length, skew and fat tails (Bailey and López de Prado)",
    numeric: true,
    value: (live) => stat(live, (stats) => stats.probabilistic_sharpe),
    cell: (live) => <span className="num">{fmt.percent(stat(live, (stats) => stats.probabilistic_sharpe))}</span>,
  },
  verdict: {
    label: "Verdict",
    about:
      "Day N of 20 until 20 live sessions. Then off track once the run leaves the range its source backtest predicts (excess return below the 5th percentile of the backtest's rolling windows of the same age, drawdown beyond their 95th percentile, or volatility above 1.5× the backtest's), beats SPY once its confidence reaches 1 − 0.05 ÷ live runs started, no backtest to compare when it has no source backtest, otherwise on track. Click a verdict for its tests",
    value: (live) => stat(live, (stats) => stats.verdict) ?? "incubating",
    cell: (live) => <Verdict live={live} />,
  },
  sessions_needed: {
    label: "Still needed",
    about:
      "Sessions still needed before the excess return could count as proven at this confidence: the minimum track record length (Bailey and López de Prado) at 1 − 0.05 ÷ live runs started. Blank while the excess Sharpe is not positive",
    numeric: true,
    ascending: true,
    value: (live) => live.sessions_needed,
    cell: (live) => <span className="num">{fmt.integer(live.sessions_needed)}</span>,
  },
  status: {
    label: "Status",
    value: (live) => (retired(live) ? "retired" : live.run.status),
    cell: (live) => <LiveStatus live={live} />,
  },
};

export function liveSortColumns<K extends LiveKey>(keys: readonly K[]): SortColumns<Live, K> {
  return Object.fromEntries(keys.map((key) => [key, { value: LIVE_COLUMNS[key].value, descending: !LIVE_COLUMNS[key].ascending && LIVE_COLUMNS[key].numeric === true }])) as SortColumns<
    Live,
    K
  >;
}

type LiveHeaderProps<S extends string> = { sorting: Sorting<Live, S>; column: LiveKey & S; className?: string | undefined };

export function LiveHeader<S extends string>({ sorting, column, className }: LiveHeaderProps<S>) {
  const spec = LIVE_COLUMNS[column];
  return (
    <SortHeader sorting={sorting} column={column} numeric={spec.numeric ?? false} className={className} info={spec.about && <Info label={spec.label}>{spec.about}</Info>}>
      {spec.label}
    </SortHeader>
  );
}
