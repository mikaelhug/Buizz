import { useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { fmt } from "../lib/format";
import { href, navigate } from "../lib/route";
import { useResource } from "../lib/useResource";
import { confirmDialog } from "../ui/Dialog";
import { Failure, Loading } from "../ui/Feedback";
import { MetricValue } from "../ui/Metrics";
import { RunBadges, StatusPill } from "../ui/Status";
import { notify } from "../ui/Toaster";
import { BacktestDialog } from "./BacktestDialog";
import { BotActions } from "./BotActions";
import { RunView } from "./RunView";

type Run = Schemas["RunSummary"];

export function BotDetail({ name, runId }: { name: string; runId: number | undefined }) {
  const detail = useResource(() => must(api.GET("/api/bots/{name}", { params: { path: { name } } })), [name]);
  const [backtesting, setBacktesting] = useState(false);

  useSpineEvents((event) => {
    if (event.type === "run_status" && event.bot === name) {
      detail.reload();
    }
  });

  if (!detail.data) {
    return detail.error ? <Failure message={detail.error} onRetry={detail.reload} /> : <Loading />;
  }
  const { summary, runs } = detail.data;
  const selected = runId ?? summary.live_run ?? summary.stats_run ?? runs[0]?.id;
  const showRun = (run: Run) => navigate({ page: "bot", name, run: run.id });

  return (
    <div className="detail">
      <header className="detail-head">
        <h1 title={`Bot hash ${detail.data.bot_hash}`}>{summary.name}</h1>
        <span className="tf">{summary.timeframe}</span>
        <StatusPill status={summary.status} />
        {summary.holdout_runs > 0 && (
          <span className="badge badge-holdout help" title="Runs on the locked holdout period">
            Holdout ×{summary.holdout_runs}
          </span>
        )}
        <span className="detail-desc muted">{summary.description}</span>
        <span className="detail-actions">
          <span className="num detail-equity">{fmt.money(summary.equity)}</span>
          <BotActions
            bot={summary}
            onChanged={(run) => {
              detail.reload();
              showRun(run);
            }}
          />
          <button type="button" className="btn btn-sm" onClick={() => setBacktesting(true)}>
            Backtest
          </button>
        </span>
      </header>
      {selected !== undefined && <RunView key={selected} runId={selected} />}
      <RunHistory runs={runs} selected={selected} name={name} onCancelled={detail.reload} />
      {backtesting && (
        <BacktestDialog
          bot={summary}
          onClose={() => setBacktesting(false)}
          onStarted={(run) => {
            setBacktesting(false);
            detail.reload();
            showRun(run);
          }}
        />
      )}
    </div>
  );
}

function period(run: Run): string {
  return `${run.config.start} → ${run.config.end ?? "now"}`;
}

function RunHistory({ runs, selected, name, onCancelled }: { runs: Run[]; selected: number | undefined; name: string; onCancelled: () => void }) {
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
      onCancelled();
    } catch (error) {
      notify(errorMessage(error));
    }
  };

  return (
    <section className="panel">
      <h2>Runs</h2>
      {runs.length === 0 ? (
        <p className="empty">No runs yet</p>
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Run</th>
                <th>Mode</th>
                <th>Status</th>
                <th>Period</th>
                <th className="r">Return</th>
                <th className="r">Sharpe</th>
                <th className="r">Max DD</th>
                <th>Created</th>
                <th />
              </tr>
            </thead>
            <tbody>
              {runs.map((run) => (
                <tr key={run.id} className={run.id === selected ? "selected" : undefined}>
                  <td>
                    <a href={href({ page: "bot", name, run: run.id })} title={run.result_hash ? `Result hash ${run.result_hash}` : undefined}>
                      #{run.id}
                    </a>
                  </td>
                  <td>{run.mode}</td>
                  <td>
                    <StatusPill status={run.status} />
                  </td>
                  <td className="nowrap">{period(run)}</td>
                  <td className="r">
                    <MetricValue metrics={run.metrics} metric="total_return" />
                  </td>
                  <td className="r">
                    <MetricValue metrics={run.metrics} metric="sharpe" />
                  </td>
                  <td className="r">
                    <MetricValue metrics={run.metrics} metric="max_drawdown" />
                  </td>
                  <td className="nowrap muted">{fmt.dateTime(run.created_at)}</td>
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
    </section>
  );
}
