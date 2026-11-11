import { useDeferredValue, useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { ABSENT, fmt, label } from "../lib/format";
import { href } from "../lib/route";
import { type Resource, useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { StatusPill } from "../ui/Status";

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

  const stale = jobs.data?.filter((job) => job.stale).length;

  return (
    <div className="data-health">
      <header className="page-head">
        <h1>Data</h1>
        {health.data && (
          <dl className="facts">
            <div>
              <dt className="help" title="Latest daily bar visible to runs">
                Daily
              </dt>
              <dd>{fmt.market(health.data.watermarks.day)}</dd>
            </div>
            <div>
              <dt className="help" title="Latest minute bar visible to runs">
                Minute
              </dt>
              <dd>{fmt.market(health.data.watermarks.minute)}</dd>
            </div>
            {health.data.ingest_enabled ? (
              <div>
                <dt>Stale jobs</dt>
                <dd className={stale ? "down" : undefined}>{fmt.integer(stale ?? health.data.stale_jobs)}</dd>
              </div>
            ) : (
              <div>
                <dt className="help" title="Scheduled data jobs are disabled (BUIZZ_INGEST=false)">
                  Ingest
                </dt>
                <dd className="muted">off</dd>
              </div>
            )}
          </dl>
        )}
      </header>
      <StaleFailure resource={health} />
      <div className="split">
        <FeedPanel feed={feed} />
        <WarningsPanel health={health} />
      </div>
      <JobsPanel jobs={jobs} />
      <CoveragePanel health={health} />
    </div>
  );
}

function FeedPanel({ feed }: { feed: Resource<Schemas["FeedSlots"]> }) {
  if (!feed.data) {
    return <section className="panel">{feed.error ? <Failure message={feed.error} onRetry={feed.reload} /> : <Loading />}</section>;
  }
  const { cap, used, connected, error, holders } = feed.data;
  const fill = cap > 0 ? used / cap : 0;
  return (
    <section className="panel">
      <header className="panel-head">
        <h2 className="help" title="Real-time IEX minute-bar symbols in use out of the plan's cap">
          Stream slots
        </h2>
        <span className={`stream-dot stream-${connected ? "open" : "down"}`} title={connected ? "Stream connected" : (error ?? "Stream disconnected")} />
        <span className="num slots">
          {used}/{cap}
        </span>
      </header>
      <StaleFailure resource={feed} />
      <div className={`meter ${fill >= 0.9 ? "meter-hot" : ""}`} role="meter" aria-valuemin={0} aria-valuemax={cap} aria-valuenow={used} aria-label="Stream slots">
        <span style={{ width: `${Math.min(fill, 1) * 100}%` }} />
      </div>
      {error && (
        <p className="form-error" role="status">
          {error}
        </p>
      )}
      {holders.length === 0 ? (
        <p className="empty">No holders</p>
      ) : (
        <ul className="holders">
          {holders.map((holder) => (
            <li key={holder.run_id}>
              <a href={href({ page: "bot", name: holder.bot, run: holder.run_id })}>{holder.bot}</a>
              <span className="muted">#{holder.run_id}</span>
              <span className="num">{holder.symbols.length}</span>
              <span className="symbols mono" title={holder.symbols.join(" ")}>
                {holder.symbols.join(" ")}
              </span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function warningText(warning: Schemas["DataWarning"]): string {
  switch (warning.type) {
    case "coverage_gap":
      return `${warning.timeframe} gap ${warning.from} → ${warning.to}`;
    case "unexplained_move":
      return `${fmt.change(warning.change)} on ${warning.date} with no corporate action`;
    case "overlap_rejected":
      return `${warning.source}: ${warning.message}`;
  }
}

function WarningsPanel({ health }: { health: Resource<Schemas["DataHealth"]> }) {
  return (
    <section className="panel">
      <h2>Warnings</h2>
      {!health.data ? (
        health.error ? <Failure message={health.error} onRetry={health.reload} /> : <Loading />
      ) : health.data.warnings.length === 0 ? (
        <p className="empty">No warnings</p>
      ) : (
        <ul className="warnings">
          {health.data.warnings.map((warning, index) => (
            <li key={index}>
              <span className="sym">{warning.symbol}</span>
              <span className="badge badge-data_warnings">{label(warning.type)}</span>
              <span>{warningText(warning)}</span>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function JobsPanel({ jobs }: { jobs: Resource<Schemas["JobView"][]> }) {
  return (
    <section className="panel">
      <h2>Jobs</h2>
      <StaleFailure resource={jobs} />
      {!jobs.data ? (
        jobs.error ? <Failure message={jobs.error} onRetry={jobs.reload} /> : <Loading />
      ) : jobs.data.length === 0 ? (
        <p className="empty">No jobs</p>
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Job</th>
                <th>Every</th>
                <th>Outcome</th>
                <th>Message</th>
                <th>Finished</th>
                <th>Next due</th>
              </tr>
            </thead>
            <tbody>
              {jobs.data.map((job) => (
                <tr key={job.name} className={job.stale ? "stale" : undefined}>
                  <td className="nowrap">
                    {job.name}
                    {job.stale && (
                      <span className="badge badge-stale help" title="Not refreshed within its schedule">
                        stale
                      </span>
                    )}
                  </td>
                  <td>{job.every}</td>
                  <td>{job.last_outcome ? <StatusPill status={job.last_outcome} /> : ABSENT}</td>
                  <td className="reason">{job.last_message ?? ""}</td>
                  <td className="nowrap muted" title={`Started ${fmt.local(job.last_started)}`}>
                    {fmt.local(job.last_finished)}
                  </td>
                  <td className="nowrap muted">{fmt.local(job.next_due)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function CoveragePanel({ health }: { health: Resource<Schemas["DataHealth"]> }) {
  const [query, setQuery] = useState("");
  const needle = useDeferredValue(query.trim().toUpperCase());
  const rows = (health.data?.coverage ?? []).filter(
    (row) => !needle || (row.symbol ?? "").toUpperCase().includes(needle) || row.dataset.toUpperCase().includes(needle),
  );
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
        </div>
      )}
    </section>
  );
}
