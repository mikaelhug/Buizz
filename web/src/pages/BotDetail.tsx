import { useLayoutEffect, useRef, useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { label, paramsText } from "../lib/format";
import { type BotTab, href, navigate } from "../lib/route";
import { PAGE_SIZE, usePaged } from "../lib/usePaged";
import { useResource } from "../lib/useResource";
import { confirmDialog } from "../ui/Dialog";
import { Failure, LoadMore, Loading, StaleFailure } from "../ui/Feedback";
import { MetricValue } from "../ui/Metrics";
import { HoldoutBadge, RunBadges, StatusPill } from "../ui/Status";
import { Ago, matches, moveBetweenRows, openRow, SortHeader, type SortColumns, TableFilter, useFlash, useSort } from "../ui/Table";
import { notify } from "../ui/Toaster";
import { BacktestDialog } from "./BacktestDialog";
import { BotActions } from "./BotActions";
import { RunView } from "./RunView";
import "../styles/bot-detail.css";

type Run = Schemas["RunSummary"];

type Props = { name: string; runId: number | undefined; tab: BotTab; symbol: string | undefined };

export function BotDetail({ name, runId, tab, symbol }: Props) {
  const detail = useResource(() => must(api.GET("/api/bots/{name}", { params: { path: { name } } })), [name]);
  const [backtesting, setBacktesting] = useState(false);

  useSpineEvents((event) => {
    if (event.type === "run_status" && event.bot === name) {
      detail.reload();
    }
  });

  if (!detail.data) {
    return detail.error ? <Failure message={detail.error} onRetry={detail.reload} /> : <Loading shape="chart" />;
  }
  const { summary, runs, status, holdout_runs: holdoutRuns } = detail.data;
  const live = summary.live[0]?.run;
  const selected = runId ?? live?.id ?? runs.find((run) => run.metrics)?.id ?? runs[0]?.id;
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
        <div className="detail-title">
          <h1>{summary.name}</h1>
          <span className="tf">{summary.timeframe}</span>
          <StatusPill status={status} />
          {holdoutRuns > 0 && <HoldoutBadge runs={holdoutRuns} />}
        </div>
        <div className="detail-actions">
          <BotActions name={name} live={live} onChanged={showRun} />
          <button type="button" className="btn btn-sm btn-primary" onClick={() => setBacktesting(true)}>
            Backtest
          </button>
        </div>
        {summary.description && <Description text={summary.description} />}
      </header>
      {selected === undefined ? (
        <section className="panel detail-empty">
          <h2>No runs yet</h2>
          <p className="muted">Backtest {summary.name} on history to see its equity, metrics and trades, or start a live paper run.</p>
          <button type="button" className="btn btn-primary" onClick={() => setBacktesting(true)}>
            Run a backtest
          </button>
        </section>
      ) : (
        <RunView
          key={selected}
          name={name}
          runId={selected}
          pinned={runId !== undefined}
          tab={tab}
          symbol={symbol}
          botHash={detail.data.bot_hash}
          recent={runs}
          runs={<RunHistory selected={selected} name={name} />}
        />
      )}
      {backtesting && (
        <BacktestDialog
          name={name}
          holdoutRuns={holdoutRuns}
          risk={detail.data.manifest.risk ?? {}}
          onClose={() => setBacktesting(false)}
          onStarted={(run) => {
            setBacktesting(false);
            showRun(run);
          }}
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

function RunHistory({ selected, name }: { selected: number; name: string }) {
  const history = usePaged(async (before) => {
    const items = await must(api.GET("/api/runs", { params: { query: { bot: name, limit: PAGE_SIZE, ...(before === undefined ? {} : { before }) } } }));
    return { items, next_after: items.length === PAGE_SIZE ? (items.at(-1)?.id ?? null) : null };
  }, [name]);
  const sorting = useSort("runs", RUN_COLUMNS, { key: "created", descending: true });
  const flash = useFlash(history.data?.items, (run) => run.id);
  const [query, setQuery] = useState("");

  useSpineEvents((event) => {
    if (event.type === "run_status" && event.bot === name) {
      history.reload();
    }
  });

  const cancel = async (run: Run) => {
    const confirmed = await confirmDialog({
      title: `Cancel backtest #${run.id}?`,
      message: `${period(run)}, ${run.status}`,
      confirmLabel: "Cancel run",
      danger: true,
    });
    if (!confirmed) {
      return;
    }
    try {
      await must(api.POST("/api/runs/{id}/cancel", { params: { path: { id: run.id } } }));
      history.reload();
    } catch (error) {
      notify(errorMessage(error));
    }
  };

  const loaded = history.data?.items ?? [];
  const varied = new Set(loaded.map((run) => paramsText(run.config.params))).size > 1;
  const shown = sorting.order(loaded.filter((run) => matches(query, `#${run.id}`, run.mode, label(run.status), period(run), paramsText(run.config.params))));

  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Runs</h2>
        <span className="muted">Every run of {name}</span>
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
                <SortHeader sorting={sorting} column="run">
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
                      {run.mode === "backtest" && (run.status === "queued" || run.status === "starting" || run.status === "running") && (
                        <button type="button" className="btn btn-xs btn-ghost" onClick={() => void cancel(run)}>
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
      <LoadMore
        onClick={flash.quietly(history.loadMore)}
        loaded={loaded.length}
        noun="runs"
      />
    </section>
  );
}
