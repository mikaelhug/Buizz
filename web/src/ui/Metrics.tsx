import type { Schemas } from "../api/client";
import { fmt, tone } from "../lib/format";

export type Metrics = Schemas["Metrics"];
export type MetricKey = keyof Metrics;

type MetricSpec = {
  key: MetricKey;
  label: string;
  title?: string;
  render: (value: number | null | undefined) => string;
  tone?: (value: number | null | undefined) => "up" | "down" | undefined;
};

const drawdownTone = (value: number | null | undefined) => (value ? "down" : undefined);

export const METRICS: Record<MetricKey, MetricSpec> = {
  total_return: { key: "total_return", label: "Return", render: fmt.change, tone },
  cagr: { key: "cagr", label: "CAGR", render: fmt.change, tone },
  volatility: { key: "volatility", label: "Vol", title: "Annualised volatility of daily returns", render: fmt.percent },
  sharpe: { key: "sharpe", label: "Sharpe", title: "Annualised, excess over the Treasury rate", render: fmt.ratio, tone },
  sortino: { key: "sortino", label: "Sortino", render: fmt.ratio, tone },
  max_drawdown: { key: "max_drawdown", label: "Max DD", title: "Largest peak-to-trough equity decline", render: fmt.percent, tone: drawdownTone },
  calmar: { key: "calmar", label: "Calmar", title: "CAGR divided by max drawdown", render: fmt.ratio },
  win_rate: { key: "win_rate", label: "Win rate", title: "Share of closed trades with a profit", render: fmt.percent },
  profit_factor: { key: "profit_factor", label: "Profit factor", title: "Gross profit divided by gross loss", render: fmt.ratio },
  exposure: { key: "exposure", label: "Exposure", title: "Average gross exposure as a share of equity", render: fmt.percent },
  turnover: { key: "turnover", label: "Turnover", title: "Annualised traded value as a multiple of equity", render: fmt.ratio },
  alpha: { key: "alpha", label: "Alpha", title: "Annualised alpha against the benchmark", render: fmt.change, tone },
  beta: { key: "beta", label: "Beta", title: "Beta against the benchmark", render: fmt.ratio },
  trades: { key: "trades", label: "Trades", render: fmt.integer },
  sessions: { key: "sessions", label: "Sessions", render: fmt.integer },
};

export const FEW_TRADES = 30;

const TRADING_DAYS = 252;

export function sharpeError(metrics: Metrics): number | undefined {
  const { sharpe, sessions } = metrics;
  return sharpe === null || sharpe === undefined || sessions < 2 ? undefined : Math.sqrt((TRADING_DAYS + (sharpe * sharpe) / 2) / sessions);
}

export function MetricValue({ metrics, metric }: { metrics: Metrics | null | undefined; metric: MetricKey }) {
  const spec = METRICS[metric];
  const value = metrics?.[metric];
  return <span className={`num ${spec.tone?.(value) ?? ""}`}>{spec.render(value)}</span>;
}

const BENCHMARKED: ReadonlySet<MetricKey> = new Set(["total_return", "cagr", "volatility", "sharpe", "sortino", "max_drawdown", "calmar"]);

type Benchmark = { symbol: string; metrics: Metrics };

export function MetricGrid({ metrics, benchmark }: { metrics: Metrics | null | undefined; benchmark: Benchmark | undefined }) {
  return (
    <dl className="metric-grid">
      {Object.values(METRICS).map((spec) => {
        const reference = benchmark && BENCHMARKED.has(spec.key) ? benchmark.metrics[spec.key] : undefined;
        return (
          <div key={spec.key} className="metric">
            <dt className={spec.title ? "help" : undefined} title={spec.title}>
              {spec.label}
            </dt>
            <dd>
              <MetricValue metrics={metrics} metric={spec.key} />
              {benchmark && reference !== undefined && reference !== null && (
                <span className="metric-bench" title={`${benchmark.symbol} buy and hold over the same period`}>
                  {benchmark.symbol} {spec.render(reference)}
                </span>
              )}
            </dd>
          </div>
        );
      })}
    </dl>
  );
}
