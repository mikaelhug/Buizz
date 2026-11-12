import { useDeferredValue, useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { ABSENT, fmt, label } from "../lib/format";
import { href } from "../lib/route";
import { type Resource, useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { Info } from "../ui/Popover";
import { StatusPill } from "../ui/Status";
import { Ago, matches, SortHeader, type SortColumns, TableFilter, useFlash, useSort } from "../ui/Table";
import "../styles/data.css";

const COVERAGE_ROWS_SHOWN = 200;
const HOT_FILL = 0.9;
const DAY_MS = 86_400_000;

const WARNING_KINDS: Record<Schemas["DataWarning"]["type"], string> = {
  coverage_gap: "Coverage gap",
  unexplained_move: "Unexplained move",
  overlap_rejected: "Overlap rejected",
  action_disagreement: "Sources disagree",
};

const TIMEFRAMES: Record<Schemas["Timeframe"], string> = { "1d": "Daily", "1m": "Minute" };

const share = (part: number, whole: number) => (whole > 0 ? Math.min(part / whole, 1) : 0);

export function DataHealth() {
  const health = useResource(() => must(api.GET("/api/data/health")), []);
  const jobs = useResource(() => must(api.GET("/api/data/jobs")), []);
  const feed = useResource(() => must(api.GET("/api/data/feed")), []);
  const processes = useResource(() => must(api.GET("/api/data/processes")), []);

  useSpineEvents((event) => {
    if (event.type === "job") {
      jobs.mutate((current) =>
        current.some((job) => job.name === event.job.name)
          ? current.map((job) => (job.name === event.job.name ? event.job : job))
          : [...current, event.job],
      );
      health.reload();
    } else if (event.type === "feed") {
      feed.mutate(() => event.slots);
    } else if (event.type === "usage") {
      processes.reload();
    }
  });

  const stale = jobs.data?.filter((job) => job.stale).length ?? health.data?.stale_jobs;

  return (
    <div className="data-health">
      <header className="page-head">
        <h1>Data</h1>
        {health.data && (
          <dl className="facts">
            <div>
              <dt>
                Daily
                <Info label="Daily watermark">The latest daily bar runs can see</Info>
              </dt>
              <dd>{fmt.market(health.data.watermarks.day)}</dd>
            </div>
            <div>
              <dt>
                Minute
                <Info label="Minute watermark">The latest minute bar runs can see</Info>
              </dt>
              <dd>{fmt.market(health.data.watermarks.minute)}</dd>
            </div>
            {health.data.ingest_enabled && (
              <div>
                <dt>Stale jobs</dt>
                <dd className={stale ? "down" : undefined}>{fmt.integer(stale)}</dd>
              </div>
            )}
          </dl>
        )}
      </header>
      <StaleFailure resource={health} />
      {health.data?.ingest_enabled === false && (
        <div className="callout" role="note">
          <strong>Ingest is off.</strong> Scheduled data jobs do not run while <code>BUIZZ_INGEST=false</code>; runs use the data already stored.
        </div>
      )}
      <div className="data-panels">
        <FeedPanel feed={feed} />
        <WarningsPanel health={health} />
        <BackfillPanel health={health} />
        <ProcessesPanel processes={processes} />
        <JobsPanel jobs={jobs} ingest={health.data?.ingest_enabled} />
        <CoveragePanel health={health} />
      </div>
    </div>
  );
}

function Meter({ value, max, name, tone }: { value: number; max: number; name: string; tone: "accent" | "hot" | "done" }) {
  return (
    <div className={`meter meter-${tone}`} role="meter" aria-valuemin={0} aria-valuemax={max} aria-valuenow={value} aria-label={name}>
      <span style={{ width: `${share(value, max) * 100}%` }} />
    </div>
  );
}

function FeedPanel({ feed }: { feed: Resource<Schemas["FeedSlots"]> }) {
  if (!feed.data) {
    return <section className="panel">{feed.error ? <Failure message={feed.error} onRetry={feed.reload} /> : <Loading />}</section>;
  }
  const { cap, used, error, connections, holders } = feed.data;
  const fill = share(used, cap);
  const up = connections.filter((connection) => connection.connected).length;
  const allUp = connections.length > 0 && up === connections.length;
  const state = connections.length === 0 ? "Not configured" : allUp ? "Connected" : `${up} of ${connections.length} connected`;
  const onlyConnection = connections.length === 1 ? connections[0] : undefined;
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Stream slots</h2>
        <Info label="Stream slots">
          Real-time IEX minute-bar symbols in use, out of 30 per Alpaca stream account. Intraday live runs hold slots for the symbols they trade; daily bots that decide before the close fetch their minutes without one.
        </Info>
        <span className={`feed-state ${allUp ? "up" : "down"}`}>
          <span className={`stream-dot stream-${allUp ? "open" : "down"}`} aria-hidden="true" />
          {state}
        </span>
      </header>
      <StaleFailure resource={feed} />
      <div className="meter-figure">
        <span className="display num">
          {used}
          <span className="muted">/{cap}</span>
        </span>
        <span className="muted">
          slots in use · <span className="num">{fmt.percent(fill)}</span>
        </span>
      </div>
      <Meter value={used} max={cap} name="Stream slots" tone={fill >= HOT_FILL ? "hot" : "accent"} />
      {error && (
        <p className="form-error" role="status">
          {error}
        </p>
      )}
      {connections.length > 1 && (
        <ul className="connections">
          {connections.map((connection, index) => (
            <li key={index}>
              <span className="connection-name">
                <span className={`stream-dot stream-${connection.connected ? "open" : "down"}`} aria-hidden="true" />
                Account {index + 1}
                <span className="muted">{connection.connected ? "connected" : "disconnected"}</span>
                <span className="connection-use num">
                  {connection.used}
                  <span className="muted">/{connection.cap}</span>
                </span>
              </span>
              {connection.error && (
                <span className="form-error" role="status">
                  {connection.error}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}
      {onlyConnection?.error && (
        <p className="form-error" role="status">
          {onlyConnection.error}
        </p>
      )}
      {holders.length === 0 ? (
        <p className="empty">No live run holds a slot. Start a bot live to stream its symbols.</p>
      ) : (
        <ul className="holders">
          {holders.map((holder) => (
            <li key={holder.run_id}>
              <span className="holder-name">
                <a href={href({ page: "bot", name: holder.bot, run: holder.run_id })}>{holder.bot}</a>
                <span className="muted">run #{holder.run_id}</span>
                <span className="holder-count num">
                  {holder.symbols.length} {holder.symbols.length === 1 ? "symbol" : "symbols"}
                </span>
              </span>
              <span className="symbols mono">{holder.symbols.join(" ")}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function gapDays(from: string, to: string): number {
  return Math.round((Date.parse(to) - Date.parse(from)) / DAY_MS) + 1;
}

function warningText(warning: Schemas["DataWarning"]): string {
  switch (warning.type) {
    case "coverage_gap": {
      const days = gapDays(warning.from, warning.to);
      return `${TIMEFRAMES[warning.timeframe]} bars missing ${warning.from} → ${warning.to} (${days} ${days === 1 ? "day" : "days"})`;
    }
    case "unexplained_move":
      return `${fmt.change(warning.change)} on ${warning.date} with no corporate action`;
    case "overlap_rejected":
      return `${label(warning.source)}: ${warning.message}`;
    case "action_disagreement":
      return `${label(warning.action)} on ${warning.ex_date}: Alpaca ${warning.alpaca ?? "none"}, Tiingo ${warning.tiingo ?? "none"}; ${label(warning.kept)} is used`;
  }
}

type Warning = Schemas["DataWarning"];
type WarningColumn = "symbol" | "kind" | "detail";

const WARNING_COLUMNS: SortColumns<Warning, WarningColumn> = {
  symbol: { value: (warning) => warning.symbol },
  kind: { value: (warning) => WARNING_KINDS[warning.type] },
  detail: { value: warningText },
};

function WarningsPanel({ health }: { health: Resource<Schemas["DataHealth"]> }) {
  const warnings = health.data?.warnings;
  const sorting = useSort("warnings", WARNING_COLUMNS, { key: "symbol", descending: false });
  const [query, setQuery] = useState("");
  const shown = sorting.order((warnings ?? []).filter((warning) => matches(query, warning.symbol, WARNING_KINDS[warning.type], warningText(warning))));
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Warnings</h2>
        {warnings && warnings.length > 0 && (
          <>
            <span className="count-chip num">{warnings.length}</span>
            <TableFilter label="Filter warnings" value={query} onChange={setQuery} />
          </>
        )}
      </header>
      {!warnings ? (
        health.error ? <Failure message={health.error} onRetry={health.reload} /> : <Loading />
      ) : warnings.length === 0 ? (
        <p className="empty">No warnings. Stored bars have no gaps or unexplained moves.</p>
      ) : shown.length === 0 ? (
        <p className="empty">No warnings match “{query.trim()}”.</p>
      ) : (
        <div className="table-wrap table-scroll warnings">
          <table className="table fin">
            <thead>
              <tr>
                <SortHeader sorting={sorting} column="symbol">
                  Symbol
                </SortHeader>
                <SortHeader sorting={sorting} column="kind">
                  Kind
                </SortHeader>
                <SortHeader sorting={sorting} column="detail">
                  Detail
                </SortHeader>
              </tr>
            </thead>
            <tbody>
              {shown.map((warning) => (
                <tr key={`${warning.symbol}-${warningText(warning)}`} className={`warning-${warning.type}`}>
                  <td className="sym">{warning.symbol}</td>
                  <td className="warning-kind">{WARNING_KINDS[warning.type]}</td>
                  <td className="reason">{warningText(warning)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function BackfillPanel({ health }: { health: Resource<Schemas["DataHealth"]> }) {
  const rows = health.data?.backfill ?? [];
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Backfill</h2>
        <Info label="Backfill">How many symbols each first backfill has reached, out of those it covers</Info>
      </header>
      {!health.data ? (
        health.error ? <Failure message={health.error} onRetry={health.reload} /> : <Loading />
      ) : rows.length === 0 ? (
        <p className="empty">Nothing to backfill</p>
      ) : (
        <ul className="backfill">
          {rows.map((row) => {
            const done = row.done >= row.total;
            const name = `${label(row.dataset)} (${label(row.source)})`;
            return (
              <li key={`${row.dataset}-${row.source}`}>
                <span className="backfill-name">
                  <span className="dataset">{label(row.dataset)}</span>
                  <span className="muted">{label(row.source)}</span>
                </span>
                <Meter value={row.done} max={row.total} name={name} tone={done ? "done" : "accent"} />
                <span className="backfill-figure num">
                  {fmt.integer(row.done)}/{fmt.integer(row.total)}
                  <span className={done ? "up" : "muted"}>{done ? "Complete" : row.done === 0 ? "Not started" : fmt.percent(share(row.done, row.total))}</span>
                </span>
              </li>
            );
          })}
        </ul>
      )}
    </section>
  );
}

type Job = Schemas["JobView"];
type JobColumn = "name" | "outcome" | "every" | "last" | "next" | "message";

const UNIT_SECONDS: Record<string, number> = { s: 1, m: 60, h: 3600, d: 86_400 };

function intervalSeconds(every: string): number | undefined {
  const [, count, unit] = /^(\d+)([smhd])$/.exec(every) ?? [];
  return count && unit ? Number(count) * (UNIT_SECONDS[unit] ?? 0) : undefined;
}

function stamp(at: string | null | undefined): number | undefined {
  return at ? Date.parse(at) : undefined;
}

const JOB_COLUMNS: SortColumns<Job, JobColumn> = {
  name: { value: (job) => job.name },
  outcome: { value: (job) => job.last_outcome },
  every: { value: (job) => intervalSeconds(job.every) },
  last: { value: (job) => stamp(job.last_finished), descending: true },
  next: { value: (job) => stamp(job.next_due) },
  message: { value: (job) => job.last_message },
};

function JobsPanel({ jobs, ingest }: { jobs: Resource<Job[]>; ingest: boolean | undefined }) {
  const sorting = useSort("jobs", JOB_COLUMNS, { key: "next", descending: false });
  const flash = useFlash(jobs.data, (job) => job.name);
  const scheduled = ingest !== false;
  return (
    <section className="panel jobs-panel">
      <header className="panel-head">
        <h2>Jobs</h2>
        {!scheduled && <span className="pill">Not scheduled</span>}
      </header>
      <StaleFailure resource={jobs} />
      {!jobs.data ? (
        jobs.error ? <Failure message={jobs.error} onRetry={jobs.reload} /> : <Loading shape="table" />
      ) : jobs.data.length === 0 ? (
        <p className="empty">No jobs</p>
      ) : (
        <div className="table-wrap table-scroll">
          <table className={`table fin jobs${scheduled ? "" : " jobs-off"}`}>
            <thead>
              <tr>
                <SortHeader sorting={sorting} column="name">
                  Job
                </SortHeader>
                <SortHeader sorting={sorting} column="outcome">
                  Last outcome
                </SortHeader>
                <SortHeader sorting={sorting} column="every" numeric>
                  Every
                </SortHeader>
                <SortHeader sorting={sorting} column="last">
                  Last run
                </SortHeader>
                {scheduled && (
                  <SortHeader sorting={sorting} column="next">
                    Next due
                  </SortHeader>
                )}
                <SortHeader sorting={sorting} column="message">
                  Message
                </SortHeader>
              </tr>
            </thead>
            <tbody>
              {sorting.order(jobs.data).map((job) => (
                <tr key={job.name} className={job.stale ? "stale" : undefined} {...flash.row(job.name)}>
                  <td>
                    <span className="job-name">
                      <span className="mono">{job.name}</span>
                      {job.stale && <span className="badge badge-stale">Stale</span>}
                    </span>
                  </td>
                  <td>{job.last_outcome ? <StatusPill status={job.last_outcome} /> : <span className="muted">Never run</span>}</td>
                  <td className="r num">{job.every}</td>
                  <td className="muted">{job.last_finished ? <Ago at={job.last_finished} /> : ABSENT}</td>
                  {scheduled && <td className="muted">{job.next_due ? <Ago at={job.next_due} /> : ABSENT}</td>}
                  <td className="reason">{job.last_message ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

type Footprint = Schemas["BotFootprint"];
type FootprintColumn = "bot" | "mode" | "runs" | "processes" | "seconds" | "peak";

const FOOTPRINT_COLUMNS: SortColumns<Footprint, FootprintColumn> = {
  bot: { value: (row) => row.bot },
  mode: { value: (row) => row.mode },
  runs: { value: (row) => row.runs, descending: true },
  processes: { value: (row) => row.usage.processes, descending: true },
  seconds: { value: (row) => perProcess(row), descending: true },
  peak: { value: (row) => row.usage.peak_memory, descending: true },
};

function perProcess(row: Footprint): number {
  return row.usage.seconds / Math.max(row.usage.processes, 1);
}

function footprintKey(row: Footprint): string {
  return `${row.bot}-${row.mode}`;
}

function ProcessesPanel({ processes }: { processes: Resource<Schemas["BotProcesses"]> }) {
  const sorting = useSort("processes", FOOTPRINT_COLUMNS, { key: "peak", descending: true });
  const flash = useFlash(processes.data?.footprints, footprintKey);
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Bot processes</h2>
        <Info label="Bot processes">
          Each bot's measured peak memory, Python included, and how long its processes ran. Backtests run up to the backtest slots at once; live bots start one process per session and up to the live slots start at once.
        </Info>
        {processes.data && (
          <span className="muted">
            {fmt.integer(processes.data.slots.backtest)} backtest · {fmt.integer(processes.data.slots.live)} live slots
          </span>
        )}
      </header>
      <StaleFailure resource={processes} />
      {!processes.data ? (
        processes.error ? <Failure message={processes.error} onRetry={processes.reload} /> : <Loading shape="table" />
      ) : processes.data.footprints.length === 0 ? (
        <p className="empty">No bot process has run yet.</p>
      ) : (
        <div className="table-wrap table-scroll">
          <table className="table fin">
            <thead>
              <tr>
                <SortHeader sorting={sorting} column="bot">
                  Bot
                </SortHeader>
                <SortHeader sorting={sorting} column="mode">
                  Mode
                </SortHeader>
                <SortHeader sorting={sorting} column="runs" numeric>
                  Runs
                </SortHeader>
                <SortHeader sorting={sorting} column="processes" numeric>
                  Processes
                </SortHeader>
                <SortHeader sorting={sorting} column="seconds" numeric>
                  Per process
                </SortHeader>
                <SortHeader sorting={sorting} column="peak" numeric>
                  Peak memory
                </SortHeader>
              </tr>
            </thead>
            <tbody>
              {sorting.order(processes.data.footprints).map((row) => (
                <tr key={footprintKey(row)} {...flash.row(footprintKey(row))}>
                  <td>
                    <a href={href({ page: "bot", name: row.bot })}>{row.bot}</a>
                  </td>
                  <td>{row.mode}</td>
                  <td className="r num">{fmt.integer(row.runs)}</td>
                  <td className="r num">{fmt.integer(row.usage.processes)}</td>
                  <td className="r num">{fmt.duration(perProcess(row))}</td>
                  <td className="r num">{fmt.memory(row.usage.peak_memory)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

type CoverageRow = Schemas["Coverage"];

type CoverageColumn = "dataset" | "symbol" | "source" | "first" | "last" | "updated";

const COVERAGE_COLUMNS: SortColumns<CoverageRow, CoverageColumn> = {
  dataset: { value: (row) => label(row.dataset) },
  symbol: { value: (row) => row.symbol },
  source: { value: (row) => label(row.source) },
  first: { value: (row) => row.first },
  last: { value: (row) => row.last, descending: true },
  updated: { value: (row) => Date.parse(row.updated_at), descending: true },
};

function coverageKey(row: CoverageRow): string {
  return `${row.dataset}-${row.symbol ?? ""}-${row.source}`;
}

function CoveragePanel({ health }: { health: Resource<Schemas["DataHealth"]> }) {
  const [query, setQuery] = useState("");
  const needle = useDeferredValue(query);
  const sorting = useSort("coverage", COVERAGE_COLUMNS, { key: "dataset", descending: false });
  const flash = useFlash(health.data?.coverage, coverageKey);
  const matching = sorting.order((health.data?.coverage ?? []).filter((row) => matches(needle, row.symbol, label(row.dataset), label(row.source))));
  const rows = matching.slice(0, COVERAGE_ROWS_SHOWN);
  return (
    <section className="panel coverage-panel">
      <header className="panel-head">
        <h2>Coverage</h2>
        <TableFilter label="Filter coverage" value={query} onChange={setQuery} />
      </header>
      {!health.data ? (
        health.error ? <Failure message={health.error} onRetry={health.reload} /> : <Loading />
      ) : rows.length === 0 ? (
        <p className="empty">{needle.trim() ? `No coverage matches “${needle.trim()}”.` : "No coverage"}</p>
      ) : (
        <>
          <div className="table-wrap table-scroll">
            <table className="table fin">
              <thead>
                <tr>
                  <SortHeader sorting={sorting} column="dataset">
                    Dataset
                  </SortHeader>
                  <SortHeader sorting={sorting} column="symbol">
                    Symbol
                  </SortHeader>
                  <SortHeader sorting={sorting} column="source">
                    Source
                  </SortHeader>
                  <SortHeader sorting={sorting} column="first">
                    First
                  </SortHeader>
                  <SortHeader sorting={sorting} column="last">
                    Last
                  </SortHeader>
                  <SortHeader sorting={sorting} column="updated">
                    Updated
                  </SortHeader>
                </tr>
              </thead>
              <tbody>
                {rows.map((row) => (
                  <tr key={coverageKey(row)} {...flash.row(coverageKey(row))}>
                    <td>{label(row.dataset)}</td>
                    <td className="sym">{row.symbol ?? ABSENT}</td>
                    <td>{label(row.source)}</td>
                    <td className="num">{row.first}</td>
                    <td className="num">{row.last}</td>
                    <td className="muted">
                      <Ago at={row.updated_at} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {matching.length > rows.length && (
            <p className="table-note muted">
              Showing the first {fmt.integer(rows.length)} of {fmt.integer(matching.length)} in this order; filter by symbol, dataset or source to find the rest
            </p>
          )}
        </>
      )}
    </section>
  );
}
