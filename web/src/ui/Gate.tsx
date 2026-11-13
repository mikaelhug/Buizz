import type { Schemas } from "../api/client";
import { Explain } from "./Popover";

type Check = Schemas["GateCheck"];

export const CHECKS: Record<Check, string> = {
  not_completed: "not completed",
  data_warnings: "data warnings",
  drawdown_stop: "drawdown stop",
  few_trades: "under 10 round trips",
  no_edge: "Sharpe ≤ 0",
  trails_benchmark: "CAGR at volatility-matched risk not above total-return SPY",
  deeper_drawdown: "max drawdown at volatility-matched risk deeper than total-return SPY",
  inconsistent_edge: "ahead of total-return SPY in fewer than 2 of 3 thirds",
};

export const CHECK_COUNT = Object.keys(CHECKS).length;

export const GATE_ABOUT =
  "A variant passes the screening gate on the longest preset it has run: completed with default costs on the current code, no data warnings (gaps in at most 0.5% of an index universe's member-sessions, in members the bot never ordered or held then, are minor and pass) or drawdown stop, at least 10 round trips and Sharpe above 0. At the same volatility as total-return SPY (dividends reinvested) it must also beat SPY's CAGR, draw down no deeper than SPY, and come out ahead of SPY in at least 2 of the period's 3 equal thirds. Passing is a screen, not a ranking: only live paper results rank bots.";

export function gateText(checks: Check[]): string {
  return checks.length === 0 ? "passes the gate" : `fails ${checks.length} of ${CHECK_COUNT} checks`;
}

export function GateResult({ checks, run }: { checks: Check[]; run: number }) {
  if (checks.length === 0) {
    return (
      <Explain label="Passes the gate" className="pill pill-ok explained" trigger="passes gate">
        <p>Backtest #{run} passes every check of the screening gate, so this variant may go live.</p>
        <p>{GATE_ABOUT}</p>
      </Explain>
    );
  }
  return (
    <Explain label={`Backtest #${run} fails`} className="pill pill-fails explained" trigger={`fails ${checks.length}/${CHECK_COUNT}`}>
      <ul className="gate-list">
        {checks.map((check) => (
          <li key={check}>{CHECKS[check]}</li>
        ))}
      </ul>
      <p>{GATE_ABOUT}</p>
    </Explain>
  );
}
