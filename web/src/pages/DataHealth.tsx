import { useDeferredValue, useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { ABSENT, fmt, label } from "../lib/format";
import { href } from "../lib/route";
import { type Resource, useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { Info } from "../ui/Popover";
import { StatusPill } from "../ui/Status";
import "../styles/data.css";

const COVERAGE_ROWS_SHOWN = 200;
const HOT_FILL = 0.9;
const DAY_MS = 86_400_000;

const WARNING_KINDS: Record<Schemas["DataWarning"]["type"], string> = {
  coverage_gap: "Coverage gap",
  unexplained_move: "Unexplained move",
  overlap_rejected: "Overlap rejected",
};

const TIMEFRAMES: Record<Schemas["Timeframe"], string> = { "1d": "Daily", "1m": "Minute" };

const share = (part: number, whole: number) => (whole > 0 ? Math.min(part / whole, 1) : 0);

export function DataHealth() {
  const health = useResource(() => must(api.GET("/api/data/health")), []);
  const jobs = useResource(() => must(api.GET("/api/data/jobs")), []);
  const feed = useResource(() => must(api.GET("/api/data/feed")), []);

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
      <div className="split">
        <FeedPanel feed={feed} />
        <WarningsPanel health={health} />
      </div>
      <BackfillPanel health={health} />
      <JobsPanel jobs={jobs} ingest={health.data?.ingest_enabled} />
      <CoveragePanel health={health} />
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
  const { cap, used, connected, error, holders } = feed.data;
  const fill = share(used, cap);
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Stream slots</h2>
        <Info label="Stream slots">Real-time IEX minute-bar symbols in use, out of the plan's cap. Live runs hold slots for the symbols they trade.</Info>
        <span className={`feed-state ${connected ? "up" : "down"}`}>
          <span className={`stream-dot stream-${connected ? "open" : "down"}`} aria-hidden="true" />
          {connected ? "Connected" : "Disconnected"}
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
  }
}

function WarningsPanel({ health }: { health: Resource<Schemas["DataHealth"]> }) {
  const warnings = health.data?.warnings;
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Warnings</h2>
        {warnings && warnings.length > 0 && <span className="count-chip num">{warnings.length}</span>}
      </header>
      {!warnings ? (
        health.error ? <Failure message={health.error} onRetry={health.reload} /> : <Loading />
      ) : warnings.length === 0 ? (
        <p className="empty">No warnings. Stored bars have no gaps or unexplained moves.</p>
      ) : (
        <ul className="warnings">
          {warnings.map((warning, index) => (
            <li key={index} className={`warning-${warning.type}`}>
              <span className="sym">{warning.symbol}</span>
              <span className="warning-kind">{WARNING_KINDS[warning.type]}</span>
              <span className="warning-text">{warningText(warning)}</span>
            </li>
          ))}
        </ul>
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

function JobsPanel({ jobs, ingest }: { jobs: Resource<Schemas["JobView"][]>; ingest: boolean | undefined }) {
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Jobs</h2>
        {ingest === false && <span className="pill">Not scheduled</span>}
      </header>
      <StaleFailure resource={jobs} />
      {!jobs.data ? (
        jobs.error ? <Failure message={jobs.error} onRetry={jobs.reload} /> : <Loading shape="table" />
      ) : jobs.data.length === 0 ? (
        <p className="empty">No jobs</p>
      ) : (
        <ul className={`jobs${ingest === false ? " jobs-off" : ""}`}>
          {jobs.data.map((job) => (
            <li key={job.name} className={job.stale ? "stale" : undefined}>
              <span className="job-name">
                <span className="mono">{job.name}</span>
                {job.stale && <span className="badge badge-stale">Stale</span>}
              </span>
              <span className="job-outcome">{job.last_outcome ? <StatusPill status={job.last_outcome} /> : <span className="muted">Never run</span>}</span>
              <dl className="job-when">
                <div>
                  <dt>Every</dt>
                  <dd>{job.every}</dd>
                </div>
                {job.last_finished && (
                  <div>
                    <dt>Last run</dt>
                    <dd>{fmt.local(job.last_finished)}</dd>
                  </div>
                )}
                {ingest !== false && job.next_due && (
                  <div>
                    <dt>Next due</dt>
                    <dd>{fmt.local(job.next_due)}</dd>
                  </div>
                )}
              </dl>
              {job.last_message && <p className="job-message">{job.last_message}</p>}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function CoveragePanel({ health }: { health: Resource<Schemas["DataHealth"]> }) {
  const [query, setQuery] = useState("");
  const needle = useDeferredValue(query.trim().toUpperCase());
  const matching = (health.data?.coverage ?? []).filter(
    (row) => !needle || (row.symbol ?? "").toUpperCase().includes(needle) || row.dataset.toUpperCase().includes(needle),
  );
  const rows = matching.slice(0, COVERAGE_ROWS_SHOWN);
  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Coverage</h2>
        <input
          className="input input-sm"
          type="search"
          placeholder="Filter"
          aria-label="Filter coverage"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
      </header>
      {!health.data ? (
        health.error ? <Failure message={health.error} onRetry={health.reload} /> : <Loading />
      ) : rows.length === 0 ? (
        <p className="empty">No coverage</p>
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Dataset</th>
                <th>Symbol</th>
                <th>Source</th>
                <th>First</th>
                <th>Last</th>
                <th>Updated</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((row) => (
                <tr key={`${row.dataset}-${row.symbol ?? ""}-${row.source}`}>
                  <td>{label(row.dataset)}</td>
                  <td className="sym">{row.symbol ?? ABSENT}</td>
                  <td>{label(row.source)}</td>
                  <td className="nowrap">{row.first}</td>
                  <td className="nowrap">{row.last}</td>
                  <td className="nowrap muted">{fmt.local(row.updated_at)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {matching.length > rows.length && (
            <p className="muted">
              Showing {fmt.integer(rows.length)} of {fmt.integer(matching.length)}; filter by symbol or dataset to find the rest
            </p>
          )}
        </div>
      )}
    </section>
  );
}
