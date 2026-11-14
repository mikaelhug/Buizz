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
type Contract = Schemas["CoverageContract"];
type ContractState = Schemas["ContractState"];
type Unit = Schemas["ScopeUnit"];

const DOMAINS: Record<Domain, { name: string; caption: string }> = {
  prices: { name: "Prices", caption: "what every backtest and live run trades on" },
  reference: { name: "Reference", caption: "listings, sessions, index members, companies" },
  filings: { name: "Filings & estimates", caption: "what companies reported, and when the market could know it" },
  macro: { name: "Macro & rates", caption: "the economy, interest rates and the Fed" },
  alternative: { name: "News, halts & short selling", caption: "headlines, trading halts, short positions and the cost of borrowing" },
};

const DOMAIN_ORDER: Domain[] = ["prices", "reference", "filings", "macro", "alternative"];

export const DATASETS: Record<Dataset, { name: string; caption: string }> = {
  bars1d: { name: "Daily prices", caption: "open, high, low, close and volume per session" },
  bars1m: { name: "Minute prices", caption: "intraday bars for the symbols bots use, kept current" },
  corporate_actions: { name: "Splits, dividends & mergers", caption: "corporate actions, cross-checked between sources" },
  option_chains: { name: "Option chains", caption: "every contract's quote, implied volatility, greeks and open interest, recorded before each close" },
  instruments: { name: "Listed securities", caption: "every stock and ETF with its listing dates" },
  calendar: { name: "Trading calendar", caption: "sessions, half days and closing times" },
  index_membership: { name: "S&P 500 membership", caption: "who was in the index on each day" },
  etf_holdings: { name: "ETF holdings", caption: "SPDR ETFs' daily holdings and shares outstanding, recorded daily" },
  delistings: { name: "Delistings", caption: "SEC Form 25 notices, each priced as an ended listing where Alpaca has its bars" },
  companies: { name: "Company profiles", caption: "industry codes and listing exchange per company" },
  fundamentals: { name: "Financial statements", caption: "XBRL facts from 10-K and 10-Q filings, as filed" },
  sec_filings: { name: "SEC filings", caption: "every filing with the time it was accepted" },
  earnings_releases: { name: "Earnings releases", caption: "8-K Item 2.02 filing times" },
  current_reports: { name: "8-K events", caption: "every 8-K with its item codes and acceptance time" },
  periodic_reports: { name: "10-K and 10-Q filings", caption: "every annual and quarterly report with its acceptance time" },
  press_releases: { name: "Earnings press releases", caption: "the text of each earnings release, as filed" },
  filing_documents: { name: "Filing documents", caption: "every document of each 8-K, 10-K and 10-Q, raw and as text, in the text store" },
  filing_sections: { name: "10-K and 10-Q items", caption: "each report's text split into its items, such as 1A risk factors and 7 MD&A" },
  listing_notices: { name: "Listing notices", caption: "8-K Item 3.01 exchange and rule, each with its quoted evidence" },
  earnings_guidance: { name: "Earnings guidance", caption: "Guidance in 8-K Item 2.02 releases, read by Claude Haiku and checked against its quote" },
  insider_trades: { name: "Insider trades", caption: "Form 4 purchases and sales" },
  insider_data_sets: { name: "Insider data sets", caption: "SEC's quarterly Form 4 tables, kept by accession until each company's filing list dates them" },
  estimates: { name: "Analyst estimates", caption: "consensus earnings expectations" },
  rates: { name: "Treasury rates", caption: "the daily yield curve and the T-bill rate cash earns" },
  macro: { name: "Macro series", caption: "FRED series with vintages, the CBOE volatility indices and put/call ratios" },
  fomc: { name: "FOMC meetings", caption: "decisions and the time each statement was released" },
  news: { name: "News", caption: "headlines per symbol with their publish time" },
  borrow: { name: "Short borrow", caption: "borrow fee and shares available, recorded daily" },
  short_interest: { name: "Short interest", caption: "shares held short twice a month, as FINRA published them" },
  short_volume: { name: "Short sale volume", caption: "daily off-exchange short volume reported to FINRA" },
  trading_halts: { name: "Trading halts", caption: "Nasdaq halts and resumptions with their reason, recorded as they appear" },
  fails_to_deliver: { name: "Fails to deliver", caption: "outstanding settlement fails per security, from the SEC's twice-monthly files" },
  page_views: { name: "Wikipedia page views", caption: "daily views of each S&P 500 company's article" },
};

export const SOURCES: Record<Source, { name: string; caption: string }> = {
  alpaca: { name: "Alpaca", caption: "prices, actions, listings, news, option chains" },
  alpaca_stream: { name: "Alpaca stream", caption: "live minute bars" },
  tiingo: { name: "Tiingo", caption: "price history before 2016" },
  edgar: { name: "SEC EDGAR", caption: "filings, financial statements and fails to deliver" },
  treasury: { name: "U.S. Treasury", caption: "yield curve" },
  fred: { name: "FRED", caption: "macro series with vintages" },
  fixture: { name: "Fixtures", caption: "local test data" },
  wikipedia: { name: "Wikipedia", caption: "S&P 500 change log and page views" },
  interactive_brokers: { name: "Interactive Brokers", caption: "short availability" },
  nasdaq: { name: "Nasdaq", caption: "listings and trading halts" },
  cboe: { name: "Cboe", caption: "volatility, skew and correlation indices, put/call ratios" },
  federal_reserve: { name: "Federal Reserve", caption: "FOMC calendar" },
  factset: { name: "FactSet", caption: "S&P 500 earnings insight" },
  finra: { name: "FINRA", caption: "short interest and short sale volume" },
  state_street: { name: "State Street", caption: "SPDR ETF holdings and shares outstanding" },
  curated: { name: "Curated", caption: "ticker links and actions checked by hand" },
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

const CONTRACT: Record<ContractState, { text: string; about: string }> = {
  complete: { text: "complete", about: "Everything its scope expects is stored; failed items are quarantined and shown." },
  filling: { text: "filling", about: "Its jobs are still reading what the scope expects; the end date comes from the pace measured over the last 30 days." },
  paused: { text: "ingest off", about: "Something it expects is missing, and nothing reads it because ingest is off on this spine; runs use the data already stored." },
  stalled: { text: "stalled", about: "Something it expects is missing and nothing is moving: a job is failing or late, or no progress in three days." },
};

const CONTRACT_RANK: ContractState[] = ["stalled", "filling", "paused", "complete"];

const UNITS: Record<Unit, string> = {
  symbols: "symbols",
  series: "series",
  lists: "lists",
  days: "days",
  sessions: "sessions",
  settlements: "settlements",
  files: "files",
  articles: "articles",
  filings: "filings",
  sections: "sections",
  quarters: "quarters",
};

function contractsOf(coverage: Coverage | undefined, source?: Source): Contract[] {
  return (coverage?.contracts ?? []).filter((item) => source === undefined || item.source === source);
}

function headline(contracts: Contract[]): Contract | undefined {
  return contracts.toSorted((a, b) => CONTRACT_RANK.indexOf(a.state) - CONTRACT_RANK.indexOf(b.state) || (b.eta ?? "").localeCompare(a.eta ?? ""))[0];
}

const shortDate = new Intl.DateTimeFormat("en-GB", { day: "numeric", month: "short", year: "numeric", timeZone: "UTC" });
const dayOf = (date: string) => shortDate.format(new Date(`${date}T00:00:00Z`));

const DAYS_PER_MONTH = 30;

function paceText(perDay: number, unit: Unit): string {
  return perDay >= 1 ? `${fmt.integer(Math.round(perDay))} ${UNITS[unit]}/day` : `${fmt.integer(Math.round(perDay * DAYS_PER_MONTH))} ${UNITS[unit]}/month`;
}

function contractText(contract: Contract): string {
  if (contract.state === "filling") {
    return contract.eta ? `filling, done ${monthOf(contract.eta)}` : "filling, measuring pace";
  }
  return CONTRACT[contract.state].text;
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

function stopped(coverage: Coverage | undefined, today: string): string | undefined {
  if (!coverage?.last) {
    return undefined;
  }
  return Date.parse(today) - Date.parse(coverage.last) > STALE_DAYS * 86_400_000 ? coverage.last : undefined;
}

const dayName = new Intl.DateTimeFormat("en-GB", { month: "short", year: "numeric", timeZone: "UTC" });
const monthOf = (date: string) => dayName.format(new Date(`${date}T00:00:00Z`));

const wholePercent = (contract: Contract) => `${Math.floor((contract.done / Math.max(1, contract.expected)) * 100)}%`;

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
    } else if (event.type === "contracts") {
      spine.mutate((current) => ({
        ...current,
        datasets: current.datasets.map((entry) => ({
          ...entry,
          markets: entry.markets.map((market) => ({ ...market, contracts: event.contracts.filter((contract) => contract.dataset === entry.dataset) })),
        })),
      }));
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
      <section className={data.ingest_enabled ? "spine-board" : "spine-board ingest-off"} aria-label="Datasets">
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
              Complete
              <Info label="Complete">Each dataset declares its scope: the symbols, filings or days it should hold, from its source's own index, from when recording began, or from the layer below. The bar is how much of that scope is stored; filling shows the end date its measured pace gives, stalled means nothing is moving, and ingest off means this spine reads nothing new. Red marks are months inside a dataset's range with no data from any source.</Info>
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
  const contracts = data.datasets.flatMap((entry) => entry.markets.flatMap((market) => market.contracts));
  const complete = contracts.filter((contract) => contract.state === "complete").length;
  const stalled = contracts.filter((contract) => contract.state === "stalled").length;
  const paused = contracts.filter((contract) => contract.state === "paused").length;
  const pending = [...(stalled > 0 ? [`${fmt.integer(stalled)} stalled`] : []), ...(paused > 0 ? [`${fmt.integer(paused)} paused, ingest off`] : [])];
  return (
    <section className="spine-summary" aria-label="Spine health">
      <dl className="summary-figures">
        <Figure value={fmt.integer(data.datasets.length)} label="datasets" />
        <Figure value={fmt.integer(sources)} label="sources" />
        <Figure value={fmt.integer(symbols)} label="symbols priced" />
        <Figure value={`${fmt.integer(complete)}/${fmt.integer(contracts.length)}`} label={["complete", ...pending].join(", ")} />
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
  const contract = headline(contractsOf(coverage, focus));
  const gaps = coverage ? gapMonths(coverage.monthly).length : 0;
  const stop = stopped(coverage, today);
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
            <Spark key={market.market} name={text.name} coverage={market} start={start} peak={peak} stop={stopped(market, today)} />
          ))}
        </span>
        <span role="cell" className="col-num num">
          {coverage?.symbols ? fmt.integer(coverage.symbols) : <span className="muted">{coverage?.first ? "series" : "—"}</span>}
          {coverage?.first && <span className="leaf-range">{yearRange(coverage, today)}</span>}
        </span>
        <span role="cell" className="leaf-fill">
          <FillMeter contract={contract} empty={!coverage?.first} gaps={gaps} stop={stop} />
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

function FillMeter({ contract, empty, gaps, stop }: { contract: Contract | undefined; empty: boolean; gaps: number; stop: string | undefined }) {
  if (empty && !contract) {
    return <span className="fill-empty">empty</span>;
  }
  return (
    <span className="fill">
      {contract ? (
        <>
          <span
            className={`fill-meter contract-${contract.state}`}
            role="meter"
            aria-valuemin={0}
            aria-valuemax={contract.expected}
            aria-valuenow={contract.done}
            aria-label={`${fmt.integer(contract.done)} of ${fmt.integer(contract.expected)} ${UNITS[contract.unit]}, ${contractText(contract)}`}
            title={contract.reason ?? CONTRACT[contract.state].about}
          >
            <span className="fill-meter-bar" style={{ width: `${(contract.done / Math.max(1, contract.expected)) * 100}%` }} />
            <span className="fill-meter-text num">{contract.state === "complete" ? "100%" : wholePercent(contract)}</span>
          </span>
          <span className={`fill-state contract-${contract.state}`}>{contractText(contract)}</span>
        </>
      ) : (
        <span className="fill-untracked" title="No coverage contract is measured for this dataset on this spine">
          not tracked
        </span>
      )}
      {stop && <span className="fill-gaps fill-stop">stops {monthOf(stop)}</span>}
      {gaps > 0 && <span className="fill-gaps num">{gaps} empty mo.</span>}
    </span>
  );
}

const SPARK_HEIGHT = 20;
const YEAR_GAP = 0.6;
const TAIL_HEIGHT = 4;

function pieces(columns: { index: number; height: number }[], january: number): string {
  return columns
    .filter((column) => column.height > 0)
    .map((column) => {
      const gap = (column.index - january) % 12 === 0 ? YEAR_GAP : 0;
      return `M${column.index + gap},${(SPARK_HEIGHT - column.height).toFixed(2)}h${1 - gap}v${column.height.toFixed(2)}h-${1 - gap}z`;
    })
    .join("");
}

function Spark({ name, coverage, start, peak: scale, stop }: { name: string; coverage: Coverage; start: string; peak: number; stop: string | undefined }) {
  const { monthly, first, last, symbols } = coverage;
  const months = monthly.length;
  const peak = Math.max(1, ...monthly);
  const firstIndex = monthly.findIndex((count) => count > 0);
  const lastIndex = monthly.findLastIndex((count) => count > 0);
  const gaps = gapMonths(monthly);
  const series = symbols === 0;
  const january = (12 - new Date(start).getUTCMonth()) % 12;
  const height = (count: number) => (count <= 0 ? 0 : series ? SPARK_HEIGHT : Math.max(3, (Math.log1p(count) / Math.log1p(scale)) * SPARK_HEIGHT));
  const every = monthly.map((_, index) => index);
  const bed = pieces(every.map((index) => ({ index, height: SPARK_HEIGHT })), january);
  const filled = pieces(monthly.map((count, index) => ({ index, height: height(count) })), january);
  const holes = pieces(gaps.map((index) => ({ index, height: SPARK_HEIGHT })), january);
  const tail = stop && lastIndex >= 0 ? pieces(every.filter((index) => index > lastIndex).map((index) => ({ index, height: TAIL_HEIGHT })), january) : "";
  const summary = first
    ? `${name}, ${MARKETS[coverage.market].name}: ${monthAt(start, Math.max(0, firstIndex))} to ${last ?? "now"}${symbols > 0 ? `, up to ${fmt.integer(peak)} symbols in a month` : ", one series"}${gaps.length > 0 ? `, ${gaps.length} months without data` : ""}${stop ? `, nothing since ${monthOf(stop)}` : ""}`
    : `${name}, ${MARKETS[coverage.market].name}: no dated coverage stored`;
  return (
    <svg className="spark" viewBox={`0 0 ${months} ${SPARK_HEIGHT}`} preserveAspectRatio="none" role="img" aria-label={summary}>
      <title>{summary}</title>
      <path className="spark-bed" d={bed} />
      {filled && <path className={series ? "spark-piece series" : "spark-piece"} d={filled} />}
      {tail && <path className="spark-tail" d={tail} />}
      {holes && <path className="spark-gap" d={holes} />}
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
                <th scope="col">Updated</th>
              </tr>
            </thead>
            <tbody>
              {coverage.sources.map((item) => (
                <tr key={item.source}>
                  <th scope="row">{SOURCES[item.source].name}</th>
                  <td className="num">{item.symbols > 0 ? fmt.integer(item.symbols) : "series"}</td>
                  <td className="num">
                    {item.first} → {item.last}
                  </td>
                  <td>
                    <Ago at={item.updated_at} />
                  </td>
                </tr>
              ))}
              {coverage.sources.length === 0 && (
                <tr>
                  <td colSpan={4} className="muted">
                    Nothing stored yet{coverage.first ? "" : `; history starts ${monthAt(start, 0)}`}.
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        ))}
        {entry.markets.map((coverage) =>
          coverage.contracts.length > 0 ? (
            <table key={`contract-${coverage.market}`} className="detail-table contract-table">
              <thead>
                <tr>
                  <th scope="col">Scope</th>
                  <th scope="col" className="num">
                    Expected
                  </th>
                  <th scope="col" className="num">
                    Done
                  </th>
                  <th scope="col" className="num">
                    Failed
                  </th>
                  <th scope="col" className="num">
                    Pending
                  </th>
                  <th scope="col">Complete through</th>
                  <th scope="col">State</th>
                </tr>
              </thead>
              <tbody>
                {coverage.contracts.map((contract) => (
                  <tr key={contract.source}>
                    <th scope="row">
                      {SOURCES[contract.source].name}
                      <span className="contract-scope">
                        {contract.kind.replace("_", " ")}: {contract.scope}, {contract.from} → {contract.to}
                      </span>
                      {contract.recipe && (
                        <span className="contract-scope">
                          bots read <code>ctx.facts("{contract.recipe.recipe}", …)</code>:{" "}
                          {contract.recipe.versions.map((version) => `version ${version.version} ${version.state} (${version.producer})`).join(", ")}
                        </span>
                      )}
                    </th>
                    <td className="num">
                      {fmt.integer(contract.expected)} {UNITS[contract.unit]}
                    </td>
                    <td className="num">{fmt.integer(contract.done)}</td>
                    <td className="num">{contract.failed > 0 ? fmt.integer(contract.failed) : "—"}</td>
                    <td className="num">{contract.pending > 0 ? fmt.integer(contract.pending) : "—"}</td>
                    <td className="num">{contract.complete_through ? dayOf(contract.complete_through) : "—"}</td>
                    <td>
                      <span className={`state-text contract-${contract.state}`} title={CONTRACT[contract.state].about}>
                        {contractText(contract)}
                      </span>
                      {contract.per_day && contract.state === "filling" && <span className="muted num contract-pace"> {paceText(contract.per_day, contract.unit)}</span>}
                      {contract.reason && <span className="contract-reason">{contract.reason}</span>}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : null,
        )}
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

