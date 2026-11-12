import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { type SpineEvent, useSpineEvents } from "../api/stream";
import { fmt, label } from "../lib/format";
import { href, navigate } from "../lib/route";
import { useFrameBatch } from "../lib/useFrameBatch";
import { useResource } from "../lib/useResource";
import { Failure, Loading, StaleFailure } from "../ui/Feedback";
import { Icon } from "../ui/Icon";
import { compareValues, LIVE_COLUMNS, type Live, type LiveKey, nextSort, type Sort, SortHeader } from "../ui/LiveTable";
import { Info } from "../ui/Popover";
import { type Mark, Sparkline } from "../ui/Sparkline";
import { notify } from "../ui/Toaster";
import { BacktestDialog } from "./BacktestDialog";
import { BotActions, isRunning } from "./BotActions";
import "../styles/board.css";
import "../styles/bots.css";

type Bot = Schemas["BotSummary"];
type Run = Schemas["RunSummary"];
type EquityEvent = Extract<SpineEvent, { type: "equity" }>;
type LiveOrder = LiveKey | "attention";
type IdleKey = "name" | "timeframe" | "stage" | "last_run_at";

const LIVE_SHOWN: readonly LiveKey[] = ["since", "sessions", "total_return", "benchmark_return", "excess_return", "max_drawdown", "verdict"];
const STAGE_ORDER: Record<Bot["stage"], number> = { screened: 0, draft: 1, live: 2 };
const STAGE_ABOUT =
  "Draft: no completed backtest of the bot's current code. Screened: at least one completed backtest of the current code; the real screening gate comes later. Live: a live paper run is going";

function attention(live: Live): number {
  if (live.run.status === "paused" && live.run.message) {
    return 0;
  }
  return isRunning(live.run) ? 2 : 1;
}

const byAttention = (a: Live, b: Live) =>
  attention(a) - attention(b) || compareValues(LIVE_COLUMNS.confidence.value(a), LIVE_COLUMNS.confidence.value(b), true) || a.run.bot.localeCompare(b.run.bot);

const IDLE_COLUMNS: Record<IdleKey, { label: string; about?: string; value: (bot: Bot) => string | number | null | undefined; ascending: boolean }> = {
  name: { label: "Bot", value: (bot) => bot.name, ascending: true },
  timeframe: { label: "Timeframe", value: (bot) => bot.timeframe, ascending: true },
  stage: { label: "Stage", about: STAGE_ABOUT, value: (bot) => STAGE_ORDER[bot.stage], ascending: true },
  last_run_at: { label: "Last activity", value: (bot) => bot.last_run_at, ascending: false },
};

const finished = (status: Run["status"]) => status === "completed" || status === "failed" || status === "cancelled";

function patchRun(bots: Bot[], run: Run, update: (live: Live) => Live): Bot[] {
  return bots.map((bot) =>
    bot.name === run.bot && bot.live.some((live) => live.run.id === run.id) ? { ...bot, live: bot.live.map((live) => (live.run.id === run.id ? update(live) : live)) } : bot,
  );
}

export function BotGrid() {
  const bots = useResource(() => must(api.GET("/api/bots")), []);
  const [marks, setMarks] = useState<Record<number, Mark>>({});
  const [filter, setFilter] = useState("");
  const [liveSort, setLiveSort] = useState<Sort<LiveOrder>>({ key: "attention", descending: false });
  const [idleSort, setIdleSort] = useState<Sort<IdleKey>>({ key: "stage", descending: false });

  const refreshBot = (name: string) =>
    must(api.GET("/api/bots/{name}", { params: { path: { name } } })).then(
      (detail) => bots.mutate((current) => current.map((bot) => (bot.name === name ? detail.summary : bot))),
      (error: unknown) => notify(`${name}: ${errorMessage(error)}`),
    );

  const applyMarks = useFrameBatch<EquityEvent>((events) =>
    setMarks((current) => ({ ...current, ...Object.fromEntries(events.map((event) => [event.run_id, { at: event.at, equity: event.equity }])) })),
  );

  const liveRuns = (bots.data ?? []).flatMap((bot) => bot.live);
  const known = (id: number) => liveRuns.find((live) => live.run.id === id);

  const changed = (run: Run) => {
    if (finished(run.status) || !known(run.id)) {
      void refreshBot(run.bot);
    } else {
      bots.mutate((current) => patchRun(current, run, (live) => ({ ...live, run: { ...live.run, status: run.status, desired: run.desired, message: run.message ?? null } })));
    }
  };

  useSpineEvents((event) => {
    if (event.type === "live_session") {
      setMarks((current) => Object.fromEntries(Object.entries(current).filter(([id]) => Number(id) !== event.run_id)));
      if (known(event.run_id)) {
        bots.mutate((current) => patchRun(current, event.summary.run, () => event.summary));
      } else {
        void refreshBot(event.bot);
      }
    } else if (event.type === "equity" && known(event.run_id)) {
      applyMarks(event);
    } else if (event.type === "run_status" && event.mode === "live") {
      const live = known(event.run_id);
      if (!live || finished(event.status)) {
        void refreshBot(event.bot);
      } else {
        const desired = event.status === "paused" ? live.run.desired : "running";
        bots.mutate((current) => patchRun(current, live.run, (entry) => ({ ...entry, run: { ...entry.run, status: event.status, desired } })));
      }
    } else if (event.type === "run_status") {
      const now = new Date().toISOString();
      bots.mutate((current) => current.map((bot) => (bot.name === event.bot && event.status === "queued" ? { ...bot, last_run_at: now } : bot)));
      if (event.status === "completed" && bots.data?.some((bot) => bot.name === event.bot && bot.stage === "draft")) {
        void refreshBot(event.bot);
      }
    }
  });

  if (!bots.data) {
    return bots.error ? <Failure message={bots.error} onRetry={bots.reload} /> : <Loading shape="table" />;
  }
  if (bots.data.length === 0) {
    return <p className="empty">No bots yet. Add a bot folder under bots/ and it shows up here.</p>;
  }
  const query = filter.trim().toLowerCase();
  const matches = (name: string) => name.toLowerCase().includes(query);
  const liveKey = liveSort.key;
  const live = liveRuns
    .filter((entry) => matches(entry.run.bot))
    .toSorted(
      liveKey === "attention" ? byAttention : (a, b) => compareValues(LIVE_COLUMNS[liveKey].value(a), LIVE_COLUMNS[liveKey].value(b), liveSort.descending) || byAttention(a, b),
    );
  const idleColumn = IDLE_COLUMNS[idleSort.key];
  const idle = bots.data
    .filter((bot) => bot.live.length === 0 && matches(bot.name))
    .toSorted((a, b) => compareValues(idleColumn.value(a), idleColumn.value(b), idleSort.descending) || a.name.localeCompare(b.name));
  const sortLive = (key: LiveOrder) => setLiveSort((current) => nextSort(current, key, key === "attention" || LIVE_COLUMNS[key].ascending === true));

  return (
    <div className="bots">
      <header className="page-head">
        <h1>Bots</h1>
        <input className="input input-sm page-filter" type="search" placeholder="Search bots" aria-label="Search bots" value={filter} onChange={(event) => setFilter(event.target.value)} />
      </header>
      <StaleFailure resource={bots} />
      <section className="bots-section" aria-labelledby="live-heading">
        <header className="section-head">
          <h2 id="live-heading">
            Live <span className="count">{live.length}</span>
          </h2>
          {liveSort.key !== "attention" && (
            <button type="button" className="btn btn-xs btn-ghost" onClick={() => setLiveSort({ key: "attention", descending: false })}>
              Needs attention first
            </button>
          )}
        </header>
        {live.length === 0 ? (
          <p className="panel muted bots-empty">{query ? "No live run matches." : "No live runs. Go live with a bot below; only live paper results rank bots."}</p>
        ) : (
          <div className="panel table-wrap board-wrap">
            <table className="table board">
              <thead>
                <tr>
                  <SortHeader column={LIVE_COLUMNS.bot} columnKey="bot" sort={liveSort} onSort={sortLive} className="board-name" />
                  {LIVE_SHOWN.map((key) => (
                    <SortHeader key={key} column={LIVE_COLUMNS[key]} columnKey={key} sort={liveSort} onSort={sortLive} />
                  ))}
                  <th scope="col">
                    <span className="th-inner">
                      Since live vs SPY
                      <Info label="Since live vs SPY">Session-close equity since the run went live (solid) against the benchmark bought with the same cash (dashed)</Info>
                    </span>
                  </th>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {live.map((entry) => (
                  <tr key={entry.run.id} className={attention(entry) === 0 ? "attention" : undefined}>
                    <th scope="row" className="board-name">
                      {LIVE_COLUMNS.bot.cell(entry)}
                      {attention(entry) < 2 && (
                        <span className={`pill pill-${attention(entry) === 0 ? "error" : "paused"}`} title={entry.run.message ?? undefined}>
                          {attention(entry) === 0 ? "error" : "paused"}
                        </span>
                      )}
                    </th>
                    {LIVE_SHOWN.map((key) => (
                      <td key={key} className={LIVE_COLUMNS[key].numeric ? "r" : undefined}>
                        {LIVE_COLUMNS[key].cell(entry)}
                      </td>
                    ))}
                    <td className="spark-cell">
                      <Sparkline curve={entry.run.live_stats?.curve ?? []} mark={marks[entry.run.id]} benchmark={entry.run.config.benchmark} />
                    </td>
                    <td className="actions-cell">
                      <BotActions name={entry.run.bot} live={entry.run} onChanged={changed} />
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
      <section className="bots-section" aria-labelledby="idle-heading">
        <header className="section-head">
          <h2 id="idle-heading">
            Not live <span className="count">{idle.length}</span>
          </h2>
        </header>
        {idle.length > 0 && (
          <div className="panel table-wrap board-wrap">
            <table className="table board">
              <thead>
                <tr>
                  {(Object.keys(IDLE_COLUMNS) as IdleKey[]).map((key) => (
                    <SortHeader
                      key={key}
                      column={IDLE_COLUMNS[key]}
                      columnKey={key}
                      sort={idleSort}
                      onSort={(next) => setIdleSort((current) => nextSort(current, next, IDLE_COLUMNS[next].ascending))}
                      className={key === "name" ? "board-name" : undefined}
                    />
                  ))}
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {idle.map((bot) => (
                  <tr key={bot.name}>
                    <th scope="row" className="board-name">
                      <a href={href({ page: "bot", name: bot.name })}>{bot.name}</a>
                    </th>
                    <td>
                      <span className="tf">{bot.timeframe}</span>
                    </td>
                    <td>
                      <span className={`stage stage-${bot.stage}`}>{label(bot.stage)}</span>
                    </td>
                    <td className="num muted">{fmt.local(bot.last_run_at)}</td>
                    <td className="actions-cell">
                      <span className="bot-actions">
                        <BacktestButton name={bot.name} />
                        <BotActions name={bot.name} live={undefined} onChanged={changed} />
                      </span>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
      </section>
    </div>
  );
}

function BacktestButton({ name }: { name: string }) {
  const [detail, setDetail] = useState<Schemas["BotDetail"]>();
  const [busy, setBusy] = useState(false);

  const open = async () => {
    setBusy(true);
    try {
      setDetail(await must(api.GET("/api/bots/{name}", { params: { path: { name } } })));
    } catch (error) {
      notify(`${name}: ${errorMessage(error)}`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <button type="button" className="btn btn-sm" disabled={busy} onClick={() => void open()}>
        <Icon name="play" />
        Backtest
      </button>
      {detail && (
        <BacktestDialog
          name={name}
          holdoutRuns={detail.holdout_runs}
          risk={detail.manifest.risk ?? {}}
          onClose={() => setDetail(undefined)}
          onStarted={(run) => {
            setDetail(undefined);
            navigate({ page: "bot", name, run: run.id });
          }}
        />
      )}
    </>
  );
}
