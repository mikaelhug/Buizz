import { useLayoutEffect, useRef, useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt, paramsText } from "../lib/format";
import { type BotTab, href, navigate } from "../lib/route";
import { PAGE_SIZE, usePaged } from "../lib/usePaged";
import { useResource } from "../lib/useResource";
import { confirmDialog } from "../ui/Dialog";
import { Failure, LoadMore, Loading, StaleFailure } from "../ui/Feedback";
import { MetricValue } from "../ui/Metrics";
import { HoldoutBadge, RunBadges, StatusPill } from "../ui/Status";
import { notify } from "../ui/Toaster";
import { BacktestDialog } from "./BacktestDialog";
import { BotActions } from "./BotActions";
import { RunView } from "./RunView";
import "../styles/bot-detail.css";

type Run = Schemas["RunSummary"];

export function BotDetail({ name, runId, tab }: { name: string; runId: number | undefined; tab: BotTab }) {
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
  const { summary, runs } = detail.data;
  const selected = runId ?? summary.live_run ?? summary.stats_run ?? runs[0]?.id;
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
          <StatusPill status={summary.status} />
          {summary.holdout_runs > 0 && <HoldoutBadge runs={summary.holdout_runs} />}
        </div>
        <div className="detail-actions">
          <BotActions bot={summary} onChanged={showRun} />
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
          botHash={detail.data.bot_hash}
          recent={runs}
          runs={<RunHistory selected={selected} name={name} />}
        />
      )}
      {backtesting && (
        <BacktestDialog
          bot={summary}
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

function RunHistory({ selected, name }: { selected: number; name: string }) {
  const history = usePaged(async (before) => {
    const items = await must(api.GET("/api/runs", { params: { query: { bot: name, limit: PAGE_SIZE, ...(before === undefined ? {} : { before }) } } }));
    return { items, next_after: items.length === PAGE_SIZE ? (items.at(-1)?.id ?? null) : null };
  }, [name]);

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

  const varied = new Set(history.data?.items.map((run) => paramsText(run.config.params))).size > 1;

  return (
    <section className="panel">
      <header className="panel-head">
        <h2>Runs</h2>
        <span className="muted">Every run of {name}, newest first</span>
      </header>
      <StaleFailure resource={history} />
      {!history.data ? (
        history.error ? <Failure message={history.error} onRetry={history.reload} /> : <Loading shape="table" />
      ) : history.data.items.length === 0 ? (
        <p className="empty">No runs yet</p>
      ) : (
        <div className="table-wrap table-scroll">
          <table className="table fin">
            <thead>
              <tr>
                <th>Run</th>
                <th>Mode</th>
                <th>Status</th>
                <th>Period</th>
                {varied && <th>Params</th>}
                <th className="r">Return</th>
                <th className="r">Sharpe</th>
                <th className="r">Max DD</th>
                <th>Created (local)</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {history.data.items.map((run) => (
                <tr key={run.id} className={run.id === selected ? "selected" : undefined}>
                  <td>
                    <a href={href({ page: "bot", name, run: run.id })} aria-current={run.id === selected ? "true" : undefined}>
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
                  <td className="nowrap muted">{fmt.local(run.created_at)}</td>
                  <td className="badges">
                    <RunBadges badges={run.badges} />
                    {run.mode === "backtest" && (run.status === "queued" || run.status === "starting" || run.status === "running") && (
                      <button type="button" className="btn btn-xs btn-ghost" onClick={() => void cancel(run)}>
                        Cancel
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <LoadMore onClick={history.loadMore} />
    </section>
  );
}
