import { useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { useQueryParam } from "../lib/route";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { Info } from "../ui/Popover";
import { Ago, matches, PageSearch } from "../ui/Table";
import { TextVolumes } from "./TextVolumes";
import "../styles/spine.css";

type Spine = Schemas["DataSpine"];
type DatasetSpine = Schemas["DatasetSpine"];
type Coverage = Schemas["MarketCoverage"];
type Job = Schemas["JobView"];
type Feed = Schemas["FeedSlots"];
type Dataset = Schemas["Dataset"];
type Domain = Schemas["DataDomain"];
type Source = Schemas["DataSource"];
type Market = Schemas["Market"];

const DOMAINS: Record<Domain, { name: string; caption: string }> = {
  prices: { name: "Prices", caption: "what every backtest and live run trades on" },
  reference: { name: "Reference", caption: "listings, sessions, index members, companies" },
  filings: { name: "Filings & estimates", caption: "what companies reported, and when the market could know it" },
  macro: { name: "Macro & rates", caption: "the economy, interest rates and the Fed" },
  alternative: { name: "News & short selling", caption: "headlines, short positions and the cost of borrowing" },
};

const DOMAIN_ORDER: Domain[] = ["prices", "reference", "filings", "macro", "alternative"];

const DATASETS: Record<Dataset, { name: string; caption: string }> = {
  bars1d: { name: "Daily prices", caption: "open, high, low, close and volume per session" },
  bars1m: { name: "Minute prices", caption: "intraday bars from the live stream and on demand" },
  corporate_actions: { name: "Splits, dividends & mergers", caption: "corporate actions, cross-checked between sources" },
  instruments: { name: "Listed securities", caption: "every stock and ETF with its listing dates" },
  calendar: { name: "Trading calendar", caption: "sessions, half days and closing times" },
  index_membership: { name: "S&P 500 membership", caption: "who was in the index on each day" },
  companies: { name: "Company profiles", caption: "industry codes and listing exchange per company" },
  fundamentals: { name: "Financial statements", caption: "XBRL facts from 10-K and 10-Q filings, as filed" },
  sec_filings: { name: "SEC filings", caption: "every filing with the time it was accepted" },
  earnings_releases: { name: "Earnings releases", caption: "8-K Item 2.02 filing times" },
  current_reports: { name: "8-K events", caption: "every 8-K with its item codes and acceptance time" },
  press_releases: { name: "Earnings press releases", caption: "the text of each earnings release, as filed" },
  filing_documents: { name: "8-K documents", caption: "every document of each 8-K, raw and as text, in the text store" },
  insider_trades: { name: "Insider trades", caption: "Form 4 purchases and sales" },
  estimates: { name: "Analyst estimates", caption: "consensus earnings expectations" },
  rates: { name: "Treasury rates", caption: "the daily yield curve and the T-bill rate cash earns" },
  macro: { name: "Macro series", caption: "FRED series and the VIX term structure, with vintages" },
  fomc: { name: "FOMC meetings", caption: "decisions and the time each statement was released" },
  news: { name: "News", caption: "headlines per symbol with their publish time" },
  borrow: { name: "Short borrow", caption: "borrow fee and shares available, recorded daily" },
  short_interest: { name: "Short interest", caption: "shares held short twice a month, as FINRA published them" },
  short_volume: { name: "Short sale volume", caption: "daily off-exchange short volume reported to FINRA" },
};

const SOURCES: Record<Source, { name: string; caption: string }> = {
  alpaca: { name: "Alpaca", caption: "prices, actions, listings, news" },
  alpaca_stream: { name: "Alpaca stream", caption: "live minute bars" },
  tiingo: { name: "Tiingo", caption: "price history before 2016" },
  edgar: { name: "SEC EDGAR", caption: "filings and financial statements" },
  treasury: { name: "U.S. Treasury", caption: "yield curve" },
  fred: { name: "FRED", caption: "macro series with vintages" },
  fixture: { name: "Fixtures", caption: "local test data" },
  wikipedia: { name: "Wikipedia", caption: "S&P 500 change log" },
  interactive_brokers: { name: "Interactive Brokers", caption: "short availability" },
  nasdaq: { name: "Nasdaq", caption: "listings" },
  cboe: { name: "Cboe", caption: "VIX and VIX3M closes" },
  federal_reserve: { name: "Federal Reserve", caption: "FOMC calendar" },
  factset: { name: "FactSet", caption: "S&P 500 earnings insight" },
  finra: { name: "FINRA", caption: "short interest and short sale volume" },
};

const MARKETS: Record<Market, { name: string; short: string }> = {
  us: { name: "United States", short: "US" },
};

type Health = "running" | "failing" | "limited" | "late" | "healthy" | "paused" | "empty";

const HEALTH: Record<Health, { text: string; label: string; about: string }> = {
  failing: { text: "failing", label: "Failing", about: "Its last fetch failed; it is retried with backoff." },
  limited: { text: "rate limited", label: "Rate limited", about: "The source asked us to slow down; the job waits and resumes on its own." },
  late: { text: "late", label: "Late", about: "It has not succeeded for three of its intervals, so its data may be stale." },
  running: { text: "updating", label: "Updating", about: "A job is fetching new data for it right now." },
  empty: { text: "no data", label: "No data yet", about: "Nothing is stored for it yet." },
  paused: { text: "ingest off", label: "Ingest off", about: "Scheduled jobs do not run on this spine; runs use the data already stored." },
  healthy: { text: "healthy", label: "Healthy", about: "Its jobs succeed on schedule." },
};

const HEALTH_RANK: Health[] = ["failing", "limited", "late", "running", "empty", "paused", "healthy"];
const HEALTH_SHOWN: Health[] = ["healthy", "running", "late", "limited", "failing", "empty"];

const isHealth = (value: string): value is Health => value in HEALTH;
const isSource = (value: string): value is Source => value in SOURCES;

const running = (job: Job) => Boolean(job.last_started && (!job.last_finished || Date.parse(job.last_finished) < Date.parse(job.last_started)));

function jobHealth(job: Job): Health {
  if (running(job)) {
    return "running";
  }
  if (job.last_outcome === "failed") {
    return "failing";
  }
  if (job.last_outcome === "backoff") {
    return "limited";
  }
  return job.stale ? "late" : "healthy";
}

function worst(states: Health[], fallback: Health): Health {
  return HEALTH_RANK.find((state) => states.includes(state)) ?? fallback;
}

function datasetHealth(entry: DatasetSpine, jobs: Job[], feed: Feed | undefined): Health {
  const stored = entry.markets.some((market) => market.last);
  const own = jobs.filter((job) => job.datasets.includes(entry.dataset));
  if (own.length === 0) {
    if (entry.dataset === "bars1m" && feed?.error) {
      return "failing";
    }
    return stored ? "healthy" : "empty";
  }
  const state = worst(own.map((job) => jobHealth(job)), "healthy");
  const fetched = own.some((job) => job.last_outcome && job.last_outcome !== "failed");
  return state === "healthy" && !stored && !fetched ? "empty" : state;
}

function sourceHealth(source: Source, jobs: Job[], feed: Feed | undefined): Health {
  if (source === "alpaca_stream") {
    return feed?.error ? "failing" : "healthy";
  }
  return worst(jobs.filter((job) => job.source === source).map((job) => jobHealth(job)), "healthy");
}

function sourcesOf(spine: Spine, jobs: Job[]): Source[] {
  const found = new Set<Source>([...spine.datasets.flatMap((entry) => entry.markets.flatMap((market) => market.sources.map((source) => source.source))), ...jobs.flatMap((job) => (job.source ? [job.source] : []))]);
  return [...found].filter((source) => source !== "fixture" || found.size === 1).toSorted((a, b) => SOURCES[a].name.localeCompare(SOURCES[b].name));
}

function datasetSources(entry: DatasetSpine, jobs: Job[]): Source[] {
  const stored = entry.markets.flatMap((market) => market.sources.map((source) => source.source));
  const fed = jobs.filter((job) => job.datasets.includes(entry.dataset)).flatMap((job) => (job.source ? [job.source] : []));
  return [...new Set([...stored, ...fed])];
}

type Fill = { done: number; total: number };

function fillOf(coverage: Coverage | undefined, source?: Source): Fill | undefined {
  const items = (coverage?.backfill ?? []).filter((item) => source === undefined || item.source === source);
  const total = items.reduce((sum, item) => sum + item.total, 0);
  return total > 0 ? { done: items.reduce((sum, item) => sum + item.done, 0), total } : undefined;
}

function gapMonths(monthly: number[]): number[] {
  const first = monthly.findIndex((count) => count > 0);
  const last = monthly.findLastIndex((count) => count > 0);
  return first < 0 ? [] : monthly.flatMap((count, index) => (index > first && index < last && count === 0 ? [index] : []));
}

const monthName = new Intl.DateTimeFormat("en-GB", { month: "short", year: "numeric", timeZone: "UTC" });

function monthAt(start: string, index: number): string {
  const date = new Date(`${start}T00:00:00Z`);
  date.setUTCMonth(date.getUTCMonth() + index);
  return monthName.format(date);
}

function yearRange(coverage: Coverage, today: string): string {
  const end = coverage.last && coverage.last.slice(0, 4) < today.slice(0, 4) ? coverage.last.slice(0, 4) : "now";
  return `${coverage.first?.slice(0, 4) ?? ""}–${end}`;
}

const STALE_DAYS = 62;
const ON_DEMAND: Dataset[] = ["bars1m"];

function stopped(dataset: Dataset, coverage: Coverage | undefined, today: string): string | undefined {
  if (!coverage?.last || ON_DEMAND.includes(dataset)) {
    return undefined;
  }
  return Date.parse(today) - Date.parse(coverage.last) > STALE_DAYS * 86_400_000 ? coverage.last : undefined;
}

const dayName = new Intl.DateTimeFormat("en-GB", { month: "short", year: "numeric", timeZone: "UTC" });
const monthOf = (date: string) => dayName.format(new Date(`${date}T00:00:00Z`));

const wholePercent = (fill: Fill) => `${Math.floor((fill.done / fill.total) * 100)}%`;

export function DataSpine() {
  const spine = useResource(() => must(api.GET("/api/data/spine")), []);
  const jobs = useResource(() => must(api.GET("/api/data/jobs")), []);
  const feed = useResource(() => must(api.GET("/api/data/feed")), []);
  const [query, setQuery] = useQueryParam("q");
  const [sourceParam, setSourceParam] = useQueryParam("source");
  const [healthParam, setHealthParam] = useQueryParam("health");
  const [open, setOpen] = useState<Dataset>();
  const [pulses, setPulses] = useState<Record<string, number>>({});

  const pulse = (keys: string[]) => setPulses((current) => ({ ...current, ...Object.fromEntries(keys.map((key) => [key, (current[key] ?? 0) + 1])) }));

  useSpineEvents((event) => {
    if (event.type === "job") {
      const before = jobs.data?.find((job) => job.name === event.job.name);
      jobs.mutate((current) => (current.some((job) => job.name === event.job.name) ? current.map((job) => (job.name === event.job.name ? event.job : job)) : [...current, event.job]));
      pulse([...event.job.datasets, ...(event.job.source ? [`source-${event.job.source}`] : [])]);
      if (event.job.last_finished && event.job.last_finished !== before?.last_finished) {
        spine.reload();
      }
    } else if (event.type === "feed") {
      feed.mutate(() => event.slots);
      pulse(["bars1m", "source-alpaca_stream"]);
    }
  });

  if (!spine.data || !jobs.data) {
    const error = spine.error ?? jobs.error;
    return error ? <Failure message={error} onRetry={() => (spine.reload(), jobs.reload())} /> : <Loading shape="table" />;
  }
  const data = spine.data;
  const allJobs = jobs.data;
  const sources = sourcesOf(data, allJobs);
  const filters = {
    query,
    source: isSource(sourceParam) && sources.includes(sourceParam) ? sourceParam : undefined,
    health: isHealth(healthParam) ? healthParam : undefined,
  };
  const healthOf = (entry: DatasetSpine) => datasetHealth(entry, allJobs, feed.data);
  const shown = data.datasets.filter(
    (entry) =>
      (filters.source === undefined || datasetSources(entry, allJobs).includes(filters.source)) &&
      (filters.health === undefined || healthOf(entry) === filters.health) &&
      matches(filters.query, DATASETS[entry.dataset].name, DATASETS[entry.dataset].caption, ...datasetSources(entry, allJobs).map((source) => SOURCES[source].name)),
  );
  const months = Math.max(...data.datasets.flatMap((entry) => entry.markets.map((market) => market.monthly.length)), 1);
  const peak = Math.max(1, ...data.datasets.flatMap((entry) => entry.markets.flatMap((market) => market.monthly)));
  const narrowed = filters.source !== undefined || filters.health !== undefined || filters.query.trim() !== "";
  const clear = () => (setSourceParam(""), setHealthParam(""), setQuery(""));

  return (
    <div className="spine-page">
      <StaleFailure resource={spine} />
      <Summary data={data} states={data.datasets.map(healthOf)} sources={sources.length} feed={feed.data} health={filters.health} onHealth={(state) => setHealthParam(filters.health === state ? "" : (state ?? ""))} />
      <TextVolumes />
      <nav className="source-bar" aria-label="Sources">
        <span className="source-bar-label">Sources</span>
        <button type="button" className="source-pill" aria-pressed={filters.source === undefined} onClick={() => setSourceParam("")}>
          All
        </button>
        {sources.map((source) => {
          const feeds = data.datasets.filter((entry) => datasetSources(entry, allJobs).includes(source)).length;
          return (
            <button
              key={source}
              type="button"
              className="source-pill"
              aria-pressed={filters.source === source}
              title={SOURCES[source].caption}
              onClick={() => (setSourceParam(filters.source === source ? "" : source), setOpen(undefined))}
            >
              <Pulse id={`source-${source}`} pulses={pulses} />
              <HealthDot health={sourceHealth(source, allJobs, feed.data)} />
              {SOURCES[source].name}
              <span className="source-pill-count num">{feeds}</span>
            </button>
          );
        })}
      </nav>
      {filters.source && <SourcePanel source={filters.source} data={data} jobs={allJobs} feed={feed.data} onClose={() => setSourceParam("")} />}
      <section className="spine-board" aria-label="Datasets">
        <header className="spine-board-head">
          <h2>
            Datasets
            <span className="spine-board-count num">
              {shown.length === data.datasets.length ? data.datasets.length : `${shown.length} of ${data.datasets.length}`}
            </span>
          </h2>
          {narrowed && (
            <button type="button" className="btn btn-ghost btn-sm" onClick={clear}>
              Show all
            </button>
          )}
          <PageSearch label="Search datasets" value={query} onChange={setQuery} />
        </header>
        <div className="spine-grid" role="table" aria-label="Datasets by kind">
          <div className="spine-row spine-columns" role="row">
            <span role="columnheader">Dataset</span>
            <span role="columnheader" className="col-timeline">
              <Axis start={data.history_start} months={months} />
            </span>
            <span role="columnheader" className="col-num">
              Symbols
            </span>
            <span role="columnheader">
              Filled
              <Info label="Filled">How far a dataset's history backfill has come, by the symbols its jobs plan to fetch. Red marks are months inside a dataset's range with no data at all.</Info>
            </span>
            <span role="columnheader">Status</span>
          </div>
          {DOMAIN_ORDER.map((domain) => {
            const entries = shown.filter((entry) => entry.domain === domain);
            if (entries.length === 0) {
              return null;
            }
            return (
              <div key={domain} role="rowgroup" className="spine-domain" aria-label={DOMAINS[domain].name}>
                <div className="spine-row spine-domain-head" role="row">
                  <span role="cell">
                    <strong>{DOMAINS[domain].name}</strong>
                    <span className="spine-domain-caption">{DOMAINS[domain].caption}</span>
                  </span>
                  <span role="cell" className="spine-domain-health">
                    <HealthStrip states={entries.map(healthOf)} />
                  </span>
                </div>
                {entries.map((entry) => (
                  <Row
                    key={entry.dataset}
                    entry={entry}
                    start={data.history_start}
                    today={data.today}
                    peak={peak}
                    health={healthOf(entry)}
                    jobs={allJobs.filter((job) => job.datasets.includes(entry.dataset))}
                    sources={datasetSources(entry, allJobs)}
                    focus={filters.source}
                    pulses={pulses}
                    feed={entry.dataset === "bars1m" ? feed.data : undefined}
                    open={open === entry.dataset}
                    onToggle={() => setOpen(open === entry.dataset ? undefined : entry.dataset)}
                  />
                ))}
              </div>
            );
          })}
          {shown.length === 0 && (
            <p className="empty">
              Nothing matches.{" "}
              <button type="button" className="btn btn-ghost btn-sm" onClick={clear}>
                Show all datasets
              </button>
            </p>
          )}
        </div>
      </section>
    </div>
  );
}

function Pulse({ id, pulses }: { id: string; pulses: Record<string, number> }) {
  const times = pulses[id];
  return times === undefined ? null : <span key={times} className="pulse-ring" aria-hidden="true" />;
}

function HealthDot({ health }: { health: Health }) {
  return <span className={`health-dot health-${health}`} role="img" aria-label={HEALTH[health].text} title={HEALTH[health].about} />;
}

function HealthStrip({ states }: { states: Health[] }) {
  return (
    <span className="health-strip" role="img" aria-label={HEALTH_SHOWN.flatMap((state) => (states.includes(state) ? [`${states.filter((value) => value === state).length} ${HEALTH[state].text}`] : [])).join(", ")}>
      {states.toSorted((a, b) => HEALTH_RANK.indexOf(a) - HEALTH_RANK.indexOf(b)).map((state, index) => (
        <span key={index} className={`health-strip-cell health-${state}`} />
      ))}
    </span>
  );
}

type SummaryProps = { data: Spine; states: Health[]; sources: number; feed: Feed | undefined; health: Health | undefined; onHealth: (state: Health | undefined) => void };

function Summary({ data, states, sources, feed, health, onHealth }: SummaryProps) {
  const count = (state: Health) => states.filter((value) => value === state).length;
  const symbols = Math.max(0, ...data.datasets.flatMap((entry) => entry.markets.map((market) => market.symbols)));
  const connected = feed ? feed.connections.filter((connection) => connection.connected).length : 0;
  const present = HEALTH_SHOWN.filter((state) => count(state) > 0);
  return (
    <section className="spine-summary" aria-label="Spine health">
      <dl className="summary-figures">
        <Figure value={fmt.integer(data.datasets.length)} label="datasets" />
        <Figure value={fmt.integer(sources)} label="sources" />
        <Figure value={fmt.integer(symbols)} label="symbols priced" />
        <Figure value={data.markets.map((market) => MARKETS[market].short).join(" · ")} label={data.markets.length === 1 ? "market" : "markets"} />
      </dl>
      <div className="summary-health">
        <div className="health-bar" role="img" aria-label={present.map((state) => `${count(state)} ${HEALTH[state].text}`).join(", ")}>
          {present.map((state) => (
            <span key={state} className={`health-bar-part health-${state}`} style={{ flexGrow: count(state) }} />
          ))}
        </div>
        <div className="health-legend" role="group" aria-label="Show datasets by health">
          {present.map((state) => (
            <button key={state} type="button" className={`health-key health-${state}`} aria-pressed={health === state} title={HEALTH[state].about} onClick={() => onHealth(state)}>
              <HealthDot health={state} />
              <span className="num">{count(state)}</span>
              {HEALTH[state].label}
            </button>
          ))}
          {!data.ingest_enabled && (
            <span className="health-key health-paused">
              <HealthDot health="paused" />
              Ingest is off on this spine
            </span>
          )}
        </div>
      </div>
      <p className="summary-watermarks">
        <span>
          Daily prices through <strong>{fmt.market(data.watermarks.day)}</strong>
        </span>
        <span>
          minute prices through <strong>{fmt.market(data.watermarks.minute)}</strong>
        </span>
        {feed && feed.connections.length > 0 && (
          <span>
            stream{" "}
            <strong>
              {connected === feed.connections.length ? "connected" : `${connected} of ${feed.connections.length} connected`}, {fmt.integer(feed.used)} of {fmt.integer(feed.cap)} symbols
            </strong>
          </span>
        )}
        <Info label="Watermarks">The latest daily and minute bars a run can see. A live run waits at the watermark for the next bar; nothing past it is ever visible.</Info>
      </p>
    </section>
  );
}

function Figure({ value, label }: { value: string; label: string }) {
  return (
    <div className="summary-figure">
      <dt>{label}</dt>
      <dd className="num">{value}</dd>
    </div>
  );
}

function SourcePanel({ source, data, jobs, feed, onClose }: { source: Source; data: Spine; jobs: Job[]; feed: Feed | undefined; onClose: () => void }) {
  const own = jobs.filter((job) => job.source === source).toSorted((a, b) => a.name.localeCompare(b.name));
  const health = sourceHealth(source, jobs, feed);
  const coverages = data.datasets.flatMap((entry) => entry.markets.flatMap((market) => market.sources.filter((item) => item.source === source)));
  const last = coverages
    .map((item) => item.updated_at)
    .toSorted()
    .at(-1);
  const symbols = Math.max(0, ...coverages.map((item) => item.symbols));
  return (
    <section className="source-panel" aria-label={SOURCES[source].name}>
      <header className="source-panel-head">
        <HealthDot health={health} />
        <h2>{SOURCES[source].name}</h2>
        <span className="muted">{SOURCES[source].caption}</span>
        <span className={`state-text health-${health}`}>{HEALTH[health].text}</span>
        {last && (
          <span className="muted">
            updated <Ago at={last} />
          </span>
        )}
        {symbols > 0 && <span className="muted num">{fmt.integer(symbols)} symbols</span>}
        <button type="button" className="btn btn-ghost btn-sm source-panel-close" onClick={onClose}>
          All sources
        </button>
      </header>
      {own.length > 0 ? (
        <ul className="source-jobs">
          {own.map((job) => {
            const state = jobHealth(job);
            return (
              <li key={job.name} className="source-job">
                <HealthDot health={state} />
                <span className="source-job-name">{job.name.replaceAll("_", " ")}</span>
                <span className="muted">every {job.every}</span>
                <span className="muted">{job.last_finished ? <Ago at={job.last_finished} /> : "never ran"}</span>
                {job.next_due && Date.parse(job.next_due) > Date.now() && state !== "running" && (
                  <span className="muted">
                    next <Ago at={job.next_due} />
                  </span>
                )}
                {job.last_message && (
                  <span className={state === "failing" ? "source-job-message failing" : "source-job-message"} title={job.last_message}>
                    {job.last_message}
                  </span>
                )}
              </li>
            );
          })}
        </ul>
      ) : (
        <p className="muted source-jobs-none">No scheduled jobs: {source === "alpaca_stream" ? "the stream runs while live minute runs hold symbols." : "its data arrives with other jobs."}</p>
      )}
    </section>
  );
}

const YEAR_STEP = 5;

function Axis({ start, months }: { start: string; months: number }) {
  const first = Number(start.slice(0, 4));
  const years = Math.ceil(months / 12);
  const ticks = Array.from({ length: years }, (_, index) => first + index).filter((year) => (year - first) % YEAR_STEP === 0);
  return (
    <span className="spine-axis" aria-label={`Timeline from ${first} to today`}>
      {ticks.map((year) => (
        <span key={year} className="axis-tick" style={{ left: `${(((year - first) * 12) / months) * 100}%` }}>
          {year}
        </span>
      ))}
    </span>
  );
}

type RowProps = {
  entry: DatasetSpine;
  start: string;
  today: string;
  peak: number;
  health: Health;
  jobs: Job[];
  sources: Source[];
  focus: Source | undefined;
  pulses: Record<string, number>;
  feed: Feed | undefined;
  open: boolean;
  onToggle: () => void;
};

function Row({ entry, start, today, peak, health, jobs, sources, focus, pulses, feed, open, onToggle }: RowProps) {
  const text = DATASETS[entry.dataset];
  const coverage = entry.markets[0];
  const fill = fillOf(coverage, focus);
  const gaps = coverage ? gapMonths(coverage.monthly).length : 0;
  const stop = stopped(entry.dataset, coverage, today);
  const detail = `spine-detail-${entry.dataset}`;
  return (
    <>
      <div className={open ? "spine-row spine-leaf open" : "spine-row spine-leaf"} role="row">
        <Pulse id={entry.dataset} pulses={pulses} />
        <span role="cell" className="leaf-name">
          <button type="button" className="leaf-toggle" aria-expanded={open} aria-controls={detail} onClick={onToggle} title={text.caption}>
            <HealthDot health={health} />
            <span className="leaf-title">{text.name}</span>
            <span className="leaf-sources">{sources.map((source) => SOURCES[source].name).join(" · ")}</span>
          </button>
        </span>
        <span role="cell" className="col-timeline">
          {entry.markets.map((market) => (
            <Spark key={market.market} name={text.name} coverage={market} start={start} peak={peak} stop={stopped(entry.dataset, market, today)} />
          ))}
        </span>
        <span role="cell" className="col-num num">
          {coverage?.symbols ? fmt.integer(coverage.symbols) : <span className="muted">{coverage?.first ? "series" : "—"}</span>}
          {coverage?.first && <span className="leaf-range">{yearRange(coverage, today)}</span>}
        </span>
        <span role="cell" className="leaf-fill">
          <FillMeter fill={fill} empty={!coverage?.first} gaps={gaps} stop={stop} />
        </span>
        <span role="cell" className="leaf-state">
          <span className={`state-text health-${health}`}>{HEALTH[health].text}</span>
          <span className="leaf-meta">
            {feed ? (
              <span className="num">
                {fmt.integer(feed.used)}/{fmt.integer(feed.cap)} streaming
              </span>
            ) : (
              coverage?.updated_at && <Ago at={coverage.updated_at} />
            )}
          </span>
        </span>
      </div>
      {open && <Detail id={detail} entry={entry} jobs={jobs} start={start} />}
    </>
  );
}

function FillMeter({ fill, empty, gaps, stop }: { fill: Fill | undefined; empty: boolean; gaps: number; stop: string | undefined }) {
  if (empty) {
    return <span className="fill-empty">empty</span>;
  }
  return (
    <span className="fill">
      {fill ? (
        <span className={fill.done >= fill.total ? "fill-meter full" : "fill-meter"} role="meter" aria-valuemin={0} aria-valuemax={fill.total} aria-valuenow={fill.done} aria-label={`Backfill ${fmt.integer(fill.done)} of ${fmt.integer(fill.total)}`}>
          <span className="fill-meter-bar" style={{ width: `${(fill.done / fill.total) * 100}%` }} />
          <span className="fill-meter-text num">{fill.done >= fill.total ? "100%" : wholePercent(fill)}</span>
        </span>
      ) : (
        <span className="fill-untracked" title="This dataset's jobs report no backfill plan, so how complete it is cannot be measured here">
          not tracked
        </span>
      )}
      {stop && <span className="fill-gaps">stops {monthOf(stop)}</span>}
      {gaps > 0 && <span className="fill-gaps num">{gaps} empty mo.</span>}
    </span>
  );
}

const SPARK_HEIGHT = 20;

function Spark({ name, coverage, start, peak: scale, stop }: { name: string; coverage: Coverage; start: string; peak: number; stop: string | undefined }) {
  const { monthly, first, last, symbols } = coverage;
  const months = monthly.length;
  const peak = Math.max(1, ...monthly);
  const firstIndex = monthly.findIndex((count) => count > 0);
  const lastIndex = monthly.findLastIndex((count) => count > 0);
  const gaps = gapMonths(monthly);
  const series = symbols === 0;
  const height = (count: number) => (count <= 0 ? 0 : series ? SPARK_HEIGHT : Math.max(2, (Math.log1p(count) / Math.log1p(scale)) * SPARK_HEIGHT));
  const area =
    firstIndex < 0
      ? ""
      : `M${firstIndex},${SPARK_HEIGHT} ` +
        monthly
          .slice(firstIndex, lastIndex + 1)
          .map((count, offset) => `L${firstIndex + offset},${SPARK_HEIGHT - height(count)} L${firstIndex + offset + 1},${SPARK_HEIGHT - height(count)}`)
          .join(" ") +
        ` L${lastIndex + 1},${SPARK_HEIGHT} Z`;
  const summary = first
    ? `${name}, ${MARKETS[coverage.market].name}: ${monthAt(start, Math.max(0, firstIndex))} to ${last ?? "now"}${symbols > 0 ? `, up to ${fmt.integer(peak)} symbols in a month` : ", one series"}${gaps.length > 0 ? `, ${gaps.length} months without data` : ""}${stop ? `, nothing since ${monthOf(stop)}` : ""}`
    : `${name}, ${MARKETS[coverage.market].name}: no dated coverage stored`;
  return (
    <svg className="spark" viewBox={`0 0 ${months} ${SPARK_HEIGHT}`} preserveAspectRatio="none" role="img" aria-label={summary}>
      <title>{summary}</title>
      <line className="spark-base" x1={0} x2={months} y1={SPARK_HEIGHT - 0.5} y2={SPARK_HEIGHT - 0.5} />
      {area && <path className={series ? "spark-area series" : "spark-area"} d={area} />}
      {stop && lastIndex >= 0 && <rect className="spark-tail" x={lastIndex + 1} y={SPARK_HEIGHT - 4} width={Math.max(0, months - lastIndex - 1)} height={4} />}
      {gaps.map((index) => (
        <rect key={index} className="spark-gap" x={index} y={0} width={1} height={SPARK_HEIGHT} />
      ))}
    </svg>
  );
}

function Detail({ id, entry, jobs, start }: { id: string; entry: DatasetSpine; jobs: Job[]; start: string }) {
  return (
    <div id={id} className="spine-detail" role="row">
      <div role="cell" className="spine-detail-body">
        <p className="spine-detail-caption">{DATASETS[entry.dataset].caption}</p>
        {entry.markets.map((coverage) => (
          <table key={coverage.market} className="detail-table">
            <thead>
              <tr>
                <th scope="col">{MARKETS[coverage.market].short} source</th>
                <th scope="col" className="num">
                  Symbols
                </th>
                <th scope="col">Covers</th>
                <th scope="col">Backfill</th>
                <th scope="col">Updated</th>
              </tr>
            </thead>
            <tbody>
              {coverage.sources.map((item) => {
                const fill = fillOf(coverage, item.source);
                return (
                  <tr key={item.source}>
                    <th scope="row">{SOURCES[item.source].name}</th>
                    <td className="num">{item.symbols > 0 ? fmt.integer(item.symbols) : "series"}</td>
                    <td className="num">
                      {item.first} → {item.last}
                    </td>
                    <td className="num">{fill ? `${fmt.integer(fill.done)} of ${fmt.integer(fill.total)} (${wholePercent(fill)})` : "—"}</td>
                    <td>
                      <Ago at={item.updated_at} />
                    </td>
                  </tr>
                );
              })}
              {coverage.sources.length === 0 && (
                <tr>
                  <td colSpan={5} className="muted">
                    Nothing stored yet{coverage.first ? "" : `; history starts ${monthAt(start, 0)}`}.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        ))}
        {jobs.length > 0 && (
          <ul className="source-jobs">
            {jobs.map((job) => {
              const state = jobHealth(job);
              return (
                <li key={job.name} className="source-job">
                  <HealthDot health={state} />
                  <span className="source-job-name">{job.name.replaceAll("_", " ")}</span>
                  {job.source && <span className="muted">{SOURCES[job.source].name}</span>}
                  <span className="muted">every {job.every}</span>
                  <span className="muted">{job.last_finished ? <Ago at={job.last_finished} /> : "never ran"}</span>
                  {job.last_message && (
                    <span className={state === "failing" ? "source-job-message failing" : "source-job-message"} title={job.last_message}>
                      {job.last_message}
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </div>
  );
}

