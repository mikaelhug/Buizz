import { useState } from "react";
import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { useQueryParam } from "../lib/route";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { Info } from "../ui/Popover";
import { Ago, matches, PageSearch } from "../ui/Table";
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
  reference: { name: "Reference", caption: "what exists and when: listings, sessions, index members, companies" },
  filings: { name: "Company filings & estimates", caption: "what companies reported, and when the market could know it" },
  macro: { name: "Macro & rates", caption: "the economy, interest rates and the Fed" },
  alternative: { name: "News & borrow", caption: "headlines and the cost of shorting" },
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
  insider_trades: { name: "Insider trades", caption: "Form 4 purchases and sales" },
  estimates: { name: "Analyst estimates", caption: "consensus earnings expectations" },
  rates: { name: "Treasury rates", caption: "the daily yield curve and the T-bill rate cash earns" },
  macro: { name: "Macro series", caption: "FRED series and the VIX term structure, with vintages" },
  fomc: { name: "FOMC meetings", caption: "decisions and the time each statement was released" },
  news: { name: "News", caption: "headlines per symbol with their publish time" },
  borrow: { name: "Short borrow", caption: "borrow fee and shares available, recorded daily" },
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
  cboe: { name: "Cboe", caption: "VIX futures" },
  federal_reserve: { name: "Federal Reserve", caption: "FOMC calendar" },
  factset: { name: "FactSet", caption: "earnings estimates" },
};

const MARKETS: Record<Market, { name: string; short: string }> = {
  us: { name: "United States", short: "US" },
};

type Health = "running" | "failing" | "limited" | "late" | "healthy" | "paused" | "empty";

const HEALTH: Record<Health, { text: string; about: string }> = {
  running: { text: "updating now", about: "A job is fetching new data for it right now." },
  failing: { text: "failing", about: "Its last fetch failed; it is retried with backoff." },
  limited: { text: "rate limited", about: "The source asked us to slow down; the job waits and resumes on its own." },
  late: { text: "late", about: "It has not succeeded for three of its intervals, so its data may be stale." },
  healthy: { text: "healthy", about: "Its jobs succeed on schedule." },
  paused: { text: "ingest off", about: "Scheduled jobs do not run on this spine; runs use the data already stored." },
  empty: { text: "no data", about: "Nothing is stored for it yet." },
};

const HEALTH_RANK: Health[] = ["failing", "limited", "late", "running", "empty", "paused", "healthy"];

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

function holes(monthly: number[]): number {
  const first = monthly.findIndex((count) => count > 0);
  const last = monthly.findLastIndex((count) => count > 0);
  return first < 0 ? 0 : monthly.slice(first, last + 1).filter((count) => count === 0).length;
}

const monthName = new Intl.DateTimeFormat("en-GB", { month: "short", year: "numeric", timeZone: "UTC" });

function monthAt(start: string, index: number): string {
  const date = new Date(`${start}T00:00:00Z`);
  date.setUTCMonth(date.getUTCMonth() + index);
  return monthName.format(date);
}

export function DataSpine() {
  const spine = useResource(() => must(api.GET("/api/data/spine")), []);
  const jobs = useResource(() => must(api.GET("/api/data/jobs")), []);
  const feed = useResource(() => must(api.GET("/api/data/feed")), []);
  const [filter, setFilter] = useQueryParam("q");
  const [pinned, setPinned] = useState<Source>();
  const [hovered, setHovered] = useState<Source>();
  const focus = hovered ?? pinned;
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
  const shown = data.datasets.filter((entry) => matches(filter, DATASETS[entry.dataset].name, DATASETS[entry.dataset].caption, ...datasetSources(entry, allJobs).map((source) => SOURCES[source].name)));
  const states = data.datasets.map((entry) => datasetHealth(entry, allJobs, feed.data));
  const months = Math.max(...data.datasets.flatMap((entry) => entry.markets.map((market) => market.monthly.length)), 1);

  return (
    <div className="spine-page">
      <StaleFailure resource={spine} />
      <HealthSummary data={data} states={states} sources={sources.length} feed={feed.data} />
      <section className="spine-sources" aria-label="Sources">
        <header className="spine-section-head">
          <h2>Sources</h2>
          <span className="muted">where the data comes from; point at or pick one to see what it feeds</span>
        </header>
        <ul className="source-grid">
          {sources.map((source) => {
            const health = sourceHealth(source, allJobs, feed.data);
            const feeds = data.datasets.filter((entry) => datasetSources(entry, allJobs).includes(source)).length;
            const last = data.datasets
              .flatMap((entry) => entry.markets.flatMap((market) => market.sources.filter((item) => item.source === source).map((item) => item.updated_at)))
              .toSorted()
              .at(-1);
            return (
              <li key={source}>
                <button
                  type="button"
                  className={`source-tile health-${health}`}
                  aria-pressed={pinned === source}
                  onClick={() => setPinned(pinned === source ? undefined : source)}
                  onMouseEnter={() => setHovered(source)}
                  onMouseLeave={() => setHovered(undefined)}
                >
                  <Pulse id={`source-${source}`} pulses={pulses} />
                  <span className="source-name">
                    <HealthDot health={health} />
                    {SOURCES[source].name}
                  </span>
                  <span className="source-caption">{SOURCES[source].caption}</span>
                  <span className="source-meta num">
                    {feeds} {feeds === 1 ? "dataset" : "datasets"}
                    {last && (
                      <>
                        {" · "}
                        <Ago at={last} />
                      </>
                    )}
                  </span>
                </button>
              </li>
            );
          })}
        </ul>
      </section>
      <section className="spine" aria-label="Datasets">
        <header className="spine-section-head">
          <h2>Datasets</h2>
          <span className="muted">grouped by kind; each lane is one market, from {data.history_start.slice(0, 4)} to today</span>
          <PageSearch label="Search datasets" value={filter} onChange={setFilter} />
        </header>
        <div className={focus ? "spine-body focused" : "spine-body"}>
          <Axis start={data.history_start} months={months} />
          {DOMAIN_ORDER.map((domain) => {
            const entries = shown.filter((entry) => entry.domain === domain);
            if (entries.length === 0) {
              return null;
            }
            const domainStates = entries.map((entry) => datasetHealth(entry, allJobs, feed.data));
            return (
              <section key={domain} className="branch" aria-label={DOMAINS[domain].name}>
                <header className="branch-head">
                  <span className="branch-node" aria-hidden="true" />
                  <h3>{DOMAINS[domain].name}</h3>
                  <span className="branch-count num">{entries.length}</span>
                  <span className="branch-caption">{DOMAINS[domain].caption}</span>
                  <HealthDot health={worst(domainStates, "healthy")} />
                </header>
                <ul className="leaves">
                  {entries.map((entry) => (
                    <Leaf
                      key={entry.dataset}
                      entry={entry}
                      start={data.history_start}
                      health={datasetHealth(entry, allJobs, feed.data)}
                      jobs={allJobs.filter((job) => job.datasets.includes(entry.dataset))}
                      sources={datasetSources(entry, allJobs)}
                      lit={focus === undefined || datasetSources(entry, allJobs).includes(focus)}
                      pulses={pulses}
                      feed={entry.dataset === "bars1m" ? feed.data : undefined}
                      markets={data.markets.length}
                    />
                  ))}
                </ul>
              </section>
            );
          })}
          {shown.length === 0 && <p className="empty">Nothing matches “{filter.trim()}”.</p>}
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

function HealthSummary({ data, states, sources, feed }: { data: Spine; states: Health[]; sources: number; feed: Feed | undefined }) {
  const count = (state: Health) => states.filter((value) => value === state).length;
  const tiles: { state: Health; label: string }[] = [
    { state: "healthy", label: "Healthy" },
    { state: "running", label: "Updating now" },
    { state: "late", label: "Late" },
    { state: "limited", label: "Rate limited" },
    { state: "failing", label: "Failing" },
    { state: "empty", label: "No data yet" },
  ];
  const symbols = Math.max(0, ...data.datasets.flatMap((entry) => entry.markets.map((market) => market.symbols)));
  const connected = feed ? feed.connections.filter((connection) => connection.connected).length : 0;
  return (
    <section className="spine-hero" aria-label="Spine health">
      <div className="hero-figures">
        <Figure value={fmt.integer(data.datasets.length)} label="datasets" />
        <Figure value={fmt.integer(sources)} label="sources" />
        <Figure value={fmt.integer(data.markets.length)} label={data.markets.length === 1 ? `market · ${data.markets.map((market) => MARKETS[market].name).join(", ")}` : "markets"} />
        <Figure value={fmt.integer(symbols)} label="symbols with daily prices" />
      </div>
      <ul className="hero-health" aria-label="Datasets by health">
        {tiles
          .filter((tile) => tile.state === "healthy" || count(tile.state) > 0)
          .map((tile) => (
            <li key={tile.state} className={`hero-state health-${tile.state}`}>
              <HealthDot health={tile.state} />
              <span className="num">{count(tile.state)}</span>
              {tile.label}
            </li>
          ))}
        {!data.ingest_enabled && (
          <li className="hero-state health-paused">
            <HealthDot health="paused" />
            Ingest is off on this spine
          </li>
        )}
      </ul>
      <p className="hero-watermarks">
        <span>
          Runs see daily prices through <strong>{fmt.market(data.watermarks.day)}</strong>
        </span>
        <span>
          minute prices through <strong>{fmt.market(data.watermarks.minute)}</strong>
        </span>
        {feed && feed.connections.length > 0 && (
          <span>
            live stream{" "}
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
    <div className="hero-figure">
      <span className="hero-value num">{value}</span>
      <span className="hero-label">{label}</span>
    </div>
  );
}

const YEAR_STEP = 5;

function Axis({ start, months }: { start: string; months: number }) {
  const first = Number(start.slice(0, 4));
  const years = Math.ceil(months / 12);
  const ticks = Array.from({ length: years }, (_, index) => first + index).filter((year) => (year - first) % YEAR_STEP === 0);
  return (
    <div className="spine-axis" aria-hidden="true">
      <span />
      <span className="axis-track">
        {ticks.map((year) => (
          <span key={year} className="axis-tick" style={{ left: `${(((year - first) * 12) / months) * 100}%` }}>
            {year}
          </span>
        ))}
        <span className="axis-tick axis-now">now</span>
      </span>
      <span />
    </div>
  );
}

type LeafProps = {
  entry: DatasetSpine;
  start: string;
  health: Health;
  jobs: Job[];
  sources: Source[];
  lit: boolean;
  pulses: Record<string, number>;
  feed: Feed | undefined;
  markets: number;
};

function Leaf({ entry, start, health, jobs, sources, lit, pulses, feed, markets }: LeafProps) {
  const text = DATASETS[entry.dataset];
  const next = jobs
    .flatMap((job) => (job.next_due ? [job.next_due] : []))
    .toSorted()
    .at(0);
  return (
    <li className={lit ? "leaf" : "leaf dim"}>
      <Pulse id={entry.dataset} pulses={pulses} />
      <div className="leaf-name">
        <span className="leaf-title">
          <HealthDot health={health} />
          {text.name}
        </span>
        <span className="leaf-caption">{text.caption}</span>
        <span className="leaf-sources">
          {sources.map((source) => (
            <span key={source} className="source-chip">
              {SOURCES[source].name}
            </span>
          ))}
        </span>
      </div>
      <div className="leaf-lanes">
        {entry.markets.map((coverage) => (
          <Lane key={coverage.market} name={text.name} coverage={coverage} start={start} label={markets > 1 || entry.markets.length > 1} />
        ))}
      </div>
      <div className="leaf-state">
        <span className={`state-text health-${health}`}>{HEALTH[health].text}</span>
        {feed ? (
          <span className="leaf-meta num">
            stream {fmt.integer(feed.used)} of {fmt.integer(feed.cap)} symbols
          </span>
        ) : (
          entry.markets[0]?.updated_at && (
            <span className="leaf-meta">
              updated <Ago at={entry.markets[0].updated_at} />
            </span>
          )
        )}
        {next && health !== "running" && Date.parse(next) > Date.now() && (
          <span className="leaf-meta">
            next <Ago at={next} />
          </span>
        )}
      </div>
    </li>
  );
}

function Lane({ name, coverage, start, label }: { name: string; coverage: Coverage; start: string; label: boolean }) {
  const { monthly, first, last, symbols, backfill } = coverage;
  const peak = Math.max(1, ...monthly);
  const gaps = holes(monthly);
  const done = backfill.reduce((sum, item) => sum + item.done, 0);
  const total = backfill.reduce((sum, item) => sum + item.total, 0);
  const filling = total > 0 && done < total;
  const firstIndex = monthly.findIndex((count) => count > 0);
  const summary = first
    ? `${name}, ${MARKETS[coverage.market].name}: from ${monthAt(start, Math.max(0, firstIndex))} to ${last ?? "now"}${symbols > 0 ? `, up to ${fmt.integer(peak)} symbols at once` : ""}${gaps > 0 ? `, ${gaps} months without data` : ""}`
    : `${name}, ${MARKETS[coverage.market].name}: no data stored`;
  return (
    <div className="lane">
      {label && <span className="lane-market">{MARKETS[coverage.market].short}</span>}
      <svg className="lane-strip" viewBox={`0 0 ${monthly.length} 10`} preserveAspectRatio="none" shapeRendering="crispEdges" role="img" aria-label={summary}>
        <title>{summary}</title>
        <rect className="lane-bed" x={0} y={0} width={monthly.length} height={10} />
        {monthly.map((count, index) =>
          count > 0 ? <rect key={index} className="lane-month" x={index} y={0} width={1.02} height={10} style={{ opacity: 0.2 + 0.8 * Math.sqrt(count / peak) }} /> : null,
        )}
        {firstIndex >= 0 &&
          monthly.map((count, index) => (count === 0 && index > firstIndex && index < monthly.findLastIndex((value) => value > 0) ? <rect key={`gap-${index}`} className="lane-gap" x={index} y={0} width={1} height={10} /> : null))}
      </svg>
      <span className="lane-facts num">
        {first ? (
          <>
            {first.slice(0, 4)}–{last?.slice(0, 7) ?? "now"}
            {symbols > 0 && ` · ${fmt.integer(symbols)} symbols`}
            {gaps > 0 && <span className="lane-holes"> · {gaps} empty months</span>}
          </>
        ) : (
          <span className="muted">no dated coverage recorded</span>
        )}
        {filling && (
          <span className="lane-backfill">
            <span className="lane-backfill-bar" style={{ width: `${(done / total) * 100}%` }} />
            <span className="lane-backfill-text">backfill {fmt.percent(done / total).replace(/\.\d+%$/, "%")}</span>
          </span>
        )}
      </span>
    </div>
  );
}
