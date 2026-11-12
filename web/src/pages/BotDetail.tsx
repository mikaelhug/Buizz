import { useLayoutEffect, useRef, useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { label, paramsText } from "../lib/format";
import { type BotTab, href, navigate, useQueryParam } from "../lib/route";
import { useFrameBatch } from "../lib/useFrameBatch";
import { PAGE_SIZE, usePaged } from "../lib/usePaged";
import { useResource } from "../lib/useResource";
import { Breadcrumbs } from "../ui/Breadcrumbs";
import { BulkBar, CompareButton, SelectAll, SelectBox, useSelection } from "../ui/Selection";
import { confirmDialog } from "../ui/Dialog";
import { Failure, LoadMore, Loading, StaleFailure } from "../ui/Feedback";
import { MetricValue } from "../ui/Metrics";
import { HoldoutBadge, RunBadges, StatusPill } from "../ui/Status";
import { Ago, matches, moveBetweenRows, openRow, SortHeader, type SortColumns, TableFilter, useFlash, useSort } from "../ui/Table";
import { notify } from "../ui/Toaster";
import { BotActions } from "./BotActions";
import { launchBacktest, launchSweep } from "./Launch";
import { Research } from "./Research";
import { RunView } from "./RunView";
import "../styles/bot-detail.css";

type Run = Schemas["RunSummary"];

export type BotView = { runId: number | undefined; tab: BotTab; symbol: string | undefined } | { preset: string | undefined };

type Props = { name: string; view: BotView };

export function BotDetail({ name, view }: Props) {
  const detail = useResource(() => must(api.GET("/api/bots/{name}", { params: { path: { name } } })), [name]);
  const reloadOnce = useFrameBatch<unknown>(() => detail.reload());

  useSpineEvents((event) => {
    if (event.type === "run_status" && event.bot === name) {
      reloadOnce(event);
    }
  });

  if (!detail.data) {
    return detail.error ? <Failure message={detail.error} onRetry={detail.reload} /> : <Loading shape="chart" />;
  }
  const { summary, runs, status, holdout_runs: holdoutRuns } = detail.data;
  const runView = "preset" in view ? undefined : view;
  const live = summary.live.map((entry) => entry.run);
  const selected = runView && (runView.runId ?? live[0]?.id ?? summary.backtest?.run_id ?? runs.find((run) => run.status === "completed" && run.metrics)?.id ?? runs[0]?.id);
  const shownLive = live.find((run) => run.id === selected) ?? (live.length === 1 ? live[0] : undefined);
  const showRun = (run: Run) => {
    detail.reload();
    if (run.id !== selected) {
      navigate({ page: "bot", name, run: run.id });
    }
  };

  return (
    <div className="detail">
      <StaleFailure resource={detail} />
      <header className="detail-head">
        <Breadcrumbs
          trail={[
            { text: "Bots", href: href({ page: "bots" }) },
            { text: summary.name, href: href({ page: "bot", name }) },
            ...(runView ? (selected === undefined ? [] : [{ text: `Run #${selected}` }]) : [{ text: "Research" }]),
          ]}
        />
        <div className="detail-title">
          <h1>{summary.name}</h1>
          <span className="tf">{summary.timeframe}</span>
          <StatusPill status={status} />
          {holdoutRuns > 0 && <HoldoutBadge runs={holdoutRuns} />}
        </div>
        <div className="detail-actions">
          <BotActions name={name} live={shownLive} onChanged={showRun} />
          <button type="button" className="btn btn-sm" onClick={() => launchSweep(name)} aria-keyshortcuts="s">
            Sweep
          </button>
          <button type="button" className="btn btn-sm btn-primary" onClick={() => launchBacktest(name)} aria-keyshortcuts="b">
            Backtest
          </button>
        </div>
        {summary.description && <Description text={summary.description} />}
      </header>
      <nav className="view-switch" aria-label="Bot views">
        <a href={href({ page: "bot", name })} className={runView ? "active" : undefined} aria-current={runView ? "page" : undefined}>
          Runs
          {live.length > 0 && <span className="count count-live">{live.length} live</span>}
        </a>
        <a href={href({ page: "research", name })} className={runView ? undefined : "active"} aria-current={runView ? undefined : "page"}>
          Research
        </a>
      </nav>
      {!runView ? (
        <>
          <Research name={name} preset={"preset" in view ? view.preset : undefined} defaults={summary.params} live={live} />
          <RunHistory selected={undefined} name={name} />
        </>
      ) : selected === undefined ? (
        <section className="panel detail-empty">
          <h2>No runs yet</h2>
          <p className="muted">Backtest {summary.name} on history to see its equity, metrics and trades, sweep its params under Research, or start a live paper run.</p>
          <button type="button" className="btn btn-primary" onClick={() => launchBacktest(name)}>
            Run a backtest
          </button>
        </section>
      ) : (
        <RunView
          key={selected}
          name={name}
          runId={selected}
          pinned={runView.runId !== undefined}
          tab={runView.tab}
          symbol={runView.symbol}
          botHash={detail.data.bot_hash}
          recent={runs}
          liveParams={new Set(live.map((run) => JSON.stringify(run.config.params)))}
          runs={<RunHistory selected={selected} name={name} />}
        />
      )}
    </div>
  );
}

function Description({ text }: { text: string }) {
  const ref = useRef<HTMLParagraphElement>(null);
  const [open, setOpen] = useState(false);
  const [clamped, setClamped] = useState(false);

  useLayoutEffect(() => {
    const paragraph = ref.current;
    if (!paragraph || open) {
      return;
    }
    const measure = () => setClamped(paragraph.scrollHeight > paragraph.clientHeight + 1);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(paragraph);
    return () => observer.disconnect();
  }, [open, text]);

  return (
    <div className="detail-desc">
      <p ref={ref} id="bot-description" className={open ? undefined : "clamped"}>
        {text}
      </p>
      {(clamped || open) && (
        <button type="button" className="btn btn-xs btn-ghost" aria-expanded={open} aria-controls="bot-description" onClick={() => setOpen(!open)}>
          {open ? "Less" : "More"}
        </button>
      )}
    </div>
  );
}

function pending(run: Run): boolean {
  return run.mode === "backtest" && (run.status === "queued" || run.status === "starting" || run.status === "running");
}

function period(run: Run): string {
  return `${run.config.start} → ${run.config.end ?? "now"}`;
}

type RunColumn = "run" | "mode" | "status" | "period" | "params" | "return" | "sharpe" | "drawdown" | "created";

const RUN_COLUMNS: SortColumns<Run, RunColumn> = {
  run: { value: (run) => run.id, descending: true },
  mode: { value: (run) => run.mode },
  status: { value: (run) => run.status },
  period: { value: (run) => run.config.start, descending: true },
  params: { value: (run) => paramsText(run.config.params) },
  return: { value: (run) => run.metrics?.total_return, descending: true },
  sharpe: { value: (run) => run.metrics?.sharpe, descending: true },
  drawdown: { value: (run) => run.metrics?.max_drawdown, descending: true },
  created: { value: (run) => Date.parse(run.created_at), descending: true },
};

function RunHistory({ selected, name }: { selected: number | undefined; name: string }) {
  const history = usePaged(async (before) => {
    const items = await must(api.GET("/api/runs", { params: { query: { bot: name, limit: PAGE_SIZE, ...(before === undefined ? {} : { before }) } } }));
    return { items, next_after: items.length === PAGE_SIZE ? (items.at(-1)?.id ?? null) : null };
  }, [name]);
  const sorting = useSort("runs", RUN_COLUMNS, { key: "created", descending: true });
  const flash = useFlash(history.data?.items, (run) => run.id);
  const [query, setQuery] = useQueryParam("runs");
  const reloadOnce = useFrameBatch<unknown>(() => history.reload());

  useSpineEvents((event) => {
    if (event.type === "run_status" && event.bot === name) {
      reloadOnce(event);
    }
  });

  const cancel = async (runs: Run[]) => {
    const [first] = runs;
    const confirmed = await confirmDialog({
      title: runs.length === 1 && first ? `Cancel backtest #${first.id}?` : `Cancel ${runs.length} backtests?`,
      message: runs.length === 1 && first ? `${period(first)}, ${first.status}` : runs.map((run) => `#${run.id}`).join(", "),
      confirmLabel: runs.length === 1 ? "Cancel run" : "Cancel runs",
      danger: true,
    });
    if (!confirmed) {
      return;
    }
    const outcomes = await Promise.allSettled(runs.map((run) => must(api.POST("/api/runs/{id}/cancel", { params: { path: { id: run.id } }, body: {} }))));
    outcomes.forEach((outcome) => outcome.status === "rejected" && notify(errorMessage(outcome.reason)));
    chosen.clear();
    history.reload();
  };

  const loaded = history.data?.items ?? [];
  const varied = new Set(loaded.map((run) => paramsText(run.config.params))).size > 1;
  const shown = sorting.order(loaded.filter((run) => matches(query, `#${run.id}`, run.mode, label(run.status), period(run), paramsText(run.config.params))));
  const chosen = useSelection(shown.map((run) => run.id));
  const cancellable = shown.filter((run) => chosen.has(run.id) && pending(run));

  return (
    <section className="panel">
      <header className="panel-head">
        <h2>History</h2>
        <span className="muted">Every run of {name}; select runs to compare them</span>
        {loaded.length > 0 && <TableFilter label="Filter runs" value={query} onChange={setQuery} />}
      </header>
      <StaleFailure resource={history} />
      {!history.data ? (
        history.error ? <Failure message={history.error} onRetry={history.reload} /> : <Loading shape="table" />
      ) : loaded.length === 0 ? (
        <p className="empty">No runs yet</p>
      ) : shown.length === 0 ? (
        <p className="empty">No loaded runs match “{query.trim()}”.</p>
      ) : (
        <div className="table-wrap table-scroll">
          <table className="table fin">
            <thead>
              <tr>
                <SortHeader sorting={sorting} column="run" select={<SelectAll selection={chosen} label="Select every run shown" />}>
                  Run
                </SortHeader>
                <SortHeader sorting={sorting} column="mode">
                  Mode
                </SortHeader>
                <SortHeader sorting={sorting} column="status">
                  Status
                </SortHeader>
                <SortHeader sorting={sorting} column="period">
                  Period
                </SortHeader>
                {varied && (
                  <SortHeader sorting={sorting} column="params">
                    Params
                  </SortHeader>
                )}
                <SortHeader sorting={sorting} column="return" numeric>
                  Return
                </SortHeader>
                <SortHeader sorting={sorting} column="sharpe" numeric>
                  Sharpe
                </SortHeader>
                <SortHeader sorting={sorting} column="drawdown" numeric>
                  Max DD
                </SortHeader>
                <SortHeader sorting={sorting} column="created">
                  Created
                </SortHeader>
                <th>
                  <span className="visually-hidden">Badges and actions</span>
                </th>
              </tr>
            </thead>
            <tbody onKeyDown={moveBetweenRows}>
              {shown.map((run) => {
                const target = href({ page: "bot", name, run: run.id });
                return (
                  <tr key={run.id} className={run.id === selected ? "clickable selected" : "clickable"} onClick={openRow(target)} {...flash.row(run.id)}>
                    <td>
                      <SelectBox selection={chosen} value={run.id} label={`Select run #${run.id}`} />
                      <a href={target} aria-current={run.id === selected ? "true" : undefined}>
                        #{run.id}
                      </a>
                    </td>
                    <td>{run.mode}</td>
                    <td>
                      <StatusPill status={run.status} />
                    </td>
                    <td className="nowrap">{period(run)}</td>
                    {varied && <td className="mono nowrap">{paramsText(run.config.params)}</td>}
                    <td className="r">
                      <MetricValue metrics={run.metrics} metric="total_return" />
                    </td>
                    <td className="r">
                      <MetricValue metrics={run.metrics} metric="sharpe" />
                    </td>
                    <td className="r">
                      <MetricValue metrics={run.metrics} metric="max_drawdown" />
                    </td>
                    <td className="nowrap muted">
                      <Ago at={run.created_at} />
                    </td>
                    <td className="badges">
                      <RunBadges badges={run.badges} />
                      {pending(run) && (
                        <button type="button" className="btn btn-xs btn-ghost" onClick={() => void cancel([run])}>
                          Cancel
                        </button>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}
      <LoadMore onClick={flash.quietly(history.loadMore)} loaded={loaded.length} noun="runs" />
      <div className="bulk-dock">
        <BulkBar selection={chosen} noun={chosen.keys.length === 1 ? "run" : "runs"}>
          <CompareButton runs={chosen.keys} />
          {cancellable.length > 0 && (
            <button type="button" className="btn btn-sm btn-danger" onClick={() => void cancel(cancellable)}>
              Cancel {cancellable.length}
            </button>
          )}
        </BulkBar>
      </div>
    </section>
  );
}
