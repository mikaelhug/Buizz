import { api, errorMessage, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { href, useQueryParam } from "../lib/route";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { LIVE_COLUMNS, type Live, type LiveKey, LiveHeader, liveSortColumns, retired } from "../ui/LiveTable";
import { Info } from "../ui/Popover";
import { matches, PageSearch, type Sorting, useSort } from "../ui/Table";
import { notify } from "../ui/Toaster";
import "../styles/board.css";

type League = Schemas["League"];

const TRACKING_SESSIONS = 20;
const LUCK = 0.05;

const SHOWN = [
  "confidence",
  "excess_cagr",
  "excess_return",
  "total_return",
  "benchmark_return",
  "cagr",
  "sessions",
  "max_drawdown",
  "volatility",
  "beta",
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

function Group({ title, about, rows, sorting, ranked }: { title: string; about: string; rows: Live[]; sorting: Sorting<Live, Key>; ranked: boolean }) {
  return (
    <section className="bots-section" aria-label={title}>
      <header className="section-head">
        <h2>
          {title} <span className="count">{rows.length}</span>
        </h2>
        <Info label={title}>{about}</Info>
      </header>
      <div className="panel table-wrap board-wrap">
        <table className="table board">
          <thead>
            <tr>
              <LiveHeader sorting={sorting} column="bot" className="board-name" />
              <LiveHeader sorting={sorting} column="status" />
              {SHOWN.map((key) => (
                <LiveHeader key={key} sorting={sorting} column={key} />
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
                  <td key={key} className={LIVE_COLUMNS[key].numeric ? "r" : undefined}>
                    {LIVE_COLUMNS[key].cell(live)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
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
    <div className="league">
      <header className="page-head">
        <h1>League</h1>
        <PageSearch label="Search live runs" value={filter} onChange={setFilter} />
      </header>
      <StaleFailure resource={league} />
      <p className="league-luck">
        <span className="num">{fmt.integer(started)}</span> live runs started; about 5% of <span className="num">{fmt.integer(started)}</span> would look significant by luck
        <span className="muted"> (≈ {fmt.ratio(started * LUCK)})</span>
        <Info label="Luck">
          Every live run started is one more try. At a 5% significance level about one in twenty would beat the benchmark convincingly by chance alone, so a run counts as proven only once its confidence reaches 1 − 0.05 ÷ {fmt.integer(started)}.
        </Info>
      </p>
      {started === 0 ? (
        <div className="panel board-empty">
          <h2>No live runs yet</h2>
          <p className="muted">Only live paper results rank bots. Backtests screen; go live with a screened bot and it joins the league after {TRACKING_SESSIONS} sessions.</p>
          <a className="btn btn-primary btn-sm" href={href({ page: "bots" })}>
            Go to bots
          </a>
        </div>
      ) : (
        <>
          <Group
            title="Ranked"
            about={`Live runs with at least ${TRACKING_SESSIONS} sessions, running, paused or retired, ranked by the probabilistic Sharpe of daily excess return over the benchmark`}
            rows={shown(ranked)}
            sorting={sorting}
            ranked
          />
          {early.length > 0 && (
            <Group title="Too early" about={`Live runs with fewer than ${TRACKING_SESSIONS} sessions; their numbers say little yet`} rows={shown(early)} sorting={sorting} ranked={false} />
          )}
        </>
      )}
    </div>
  );
}
