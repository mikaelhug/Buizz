import { api, errorMessage, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { href, useQueryParam } from "../lib/route";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { Card, EmptyState } from "../ui/Kit";
import { LIVE_COLUMNS, type Live, type LiveKey, LiveHeader, liveSortColumns, retired } from "../ui/LiveTable";
import { Info } from "../ui/Popover";
import { matches, PageSearch, type Sorting, useSort } from "../ui/Table";
import { notify } from "../ui/Toaster";
import "../styles/board.css";

type League = Schemas["League"];

const TRACKING_SESSIONS = 20;

const SHOWN = [
  "excess_cagr",
  "confidence",
  "sessions",
  "total_return",
  "benchmark_return",
  "max_drawdown",
  "verdict",
  "sessions_needed",
] as const satisfies readonly LiveKey[];

type Key = "bot" | "status" | (typeof SHOWN)[number];

const SORT = liveSortColumns<Key>(["bot", "status", ...SHOWN]);

const tracking = (live: Live) => (live.run.live_stats?.sessions ?? 0) >= TRACKING_SESSIONS;

function place(league: League, summary: Live): League {
  const rest = (rows: Live[]) => rows.filter((row) => row.run.id !== summary.run.id);
  const known = [...league.ranked, ...league.too_early].some((row) => row.run.id === summary.run.id);
  return {
    started: league.started + (known ? 0 : 1),
    ranked: tracking(summary) ? [...rest(league.ranked), summary] : rest(league.ranked),
    too_early: tracking(summary) ? rest(league.too_early) : [...rest(league.too_early), summary],
  };
}

function patchStatus(league: League, id: number, status: Schemas["RunStatus"]): League {
  const patch = (rows: Live[]) => rows.map((row) => (row.run.id === id ? { ...row, run: { ...row.run, status } } : row));
  return { ...league, ranked: patch(league.ranked), too_early: patch(league.too_early) };
}

function Board({ label, rows, sorting, ranked }: { label: string; rows: Live[]; sorting: Sorting<Live, Key>; ranked: boolean }) {
  return (
    <div className="card table-wrap board-wrap">
      <table className="table board league-board" aria-label={label}>
        <thead>
          <tr>
            <LiveHeader sorting={sorting} column="bot" className="board-name" />
            <LiveHeader sorting={sorting} column="status" />
            {SHOWN.map((key) => (
              <LiveHeader key={key} sorting={sorting} column={key} className={key === "excess_cagr" ? "league-headline" : undefined} />
            ))}
          </tr>
        </thead>
        <tbody>
          {sorting.order(rows).map((live, index) => (
            <tr key={live.run.id} className={retired(live) ? "quiet-row" : undefined}>
              <th scope="row" className="board-name">
                {ranked && <span className="rank num">{index + 1}</span>}
                {LIVE_COLUMNS.bot.cell(live)}
              </th>
              <td>{LIVE_COLUMNS.status.cell(live)}</td>
              {SHOWN.map((key) => (
                <td key={key} className={[LIVE_COLUMNS[key].numeric ? "r" : "", key === "excess_cagr" ? "league-headline" : ""].join(" ").trim() || undefined}>
                  {LIVE_COLUMNS[key].cell(live)}
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

export function League() {
  const league = useResource(() => must(api.GET("/api/league")), []);
  const sorting = useSort("league", SORT, { key: "confidence", descending: true });
  const [filter, setFilter] = useQueryParam("q");

  const known = (id: number) => league.data && [...league.data.ranked, ...league.data.too_early].some((row) => row.run.id === id);

  useSpineEvents((event) => {
    if (event.type === "live_session") {
      league.mutate((current) => place(current, event.summary));
    } else if (event.type === "run_status" && event.mode === "live") {
      if (known(event.run_id)) {
        league.mutate((current) => patchStatus(current, event.run_id, event.status));
      } else {
        must(api.GET("/api/runs/{id}", { params: { path: { id: event.run_id } } })).then(
          (run) => league.mutate((current) => place(current, { run, sessions_needed: null })),
          (error: unknown) => notify(errorMessage(error)),
        );
      }
    }
  });

  if (!league.data) {
    return league.error ? <Failure message={league.error} onRetry={league.reload} /> : <Loading shape="table" />;
  }
  const { started, ranked, too_early: early } = league.data;
  const shown = (rows: Live[]) => rows.filter((row) => matches(filter, row.run.bot));
  return (
    <div className="page-stack league">
      <header className="page-head">
        <div>
          <h1>League</h1>
          <p className="page-question">
            Which live runs beat total-return SPY with confidence?
            <Info label="How the league ranks">
              Live runs with at least {TRACKING_SESSIONS} sessions, running, paused or retired, ranked by the probabilistic Sharpe of daily excess return over total-return SPY.
              Every live run started is one more try: at a 5% significance level about one in twenty would beat SPY convincingly by chance alone, so a run counts as proven
              only once its confidence reaches 1 − 0.05 ÷ live runs started.
            </Info>
          </p>
        </div>
        <PageSearch label="Search live runs" value={filter} onChange={setFilter} />
      </header>
      <StaleFailure resource={league} />
      {started === 0 ? (
        <Card>
          <EmptyState
            title="No live runs yet"
            action={
              <a className="btn btn-primary btn-sm" href={`${href({ page: "bots" })}?stage=screened`}>
                See screened bots
              </a>
            }
          >
            Only live paper results rank bots. Go live with a bot that passed screening and it joins the league after {TRACKING_SESSIONS} sessions.
          </EmptyState>
        </Card>
      ) : (
        <>
          <p className="league-luck">
            {fmt.integer(started)} live {started === 1 ? "run has" : "runs have"} been started. With that many tries, roughly 1 in 20 would look like a winner by luck alone,
            so a run counts as proven only at confidence 1 − 0.05 ÷ {fmt.integer(started)}.
          </p>
          <section className="league-group" aria-label="Ranked">
            {shown(ranked).length > 0 ? (
              <Board label="Ranked" rows={shown(ranked)} sorting={sorting} ranked />
            ) : (
              <Card>
                <EmptyState title={`No live run has ${TRACKING_SESSIONS} sessions yet`}>
                  {early.length > 0
                    ? `${early.length} ${early.length === 1 ? "run is" : "runs are"} incubating; each joins the ranking at session ${TRACKING_SESSIONS}.`
                    : "Nothing matches the search."}
                </EmptyState>
              </Card>
            )}
          </section>
          {early.length > 0 && (
            <details className="league-group league-early" aria-label="Too early">
              <summary>
                Too early <span className="count">{shown(early).length}</span>
                <span className="muted">fewer than {TRACKING_SESSIONS} sessions; their numbers say little yet</span>
              </summary>
              <Board label="Too early" rows={shown(early)} sorting={sorting} ranked={false} />
            </details>
          )}
        </>
      )}
    </div>
  );
}
