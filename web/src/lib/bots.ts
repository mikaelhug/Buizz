import type { Schemas } from "../api/client";
import { paramsText } from "./format";
import type { Live } from "../ui/LiveTable";

export type Bot = Schemas["BotSummary"];
export type Glance = Schemas["BacktestGlance"];

export const GROUPS = ["live", "ready", "failing", "untested"] as const;
export type Group = (typeof GROUPS)[number];

export const GROUP_TEXT: Record<Group, { name: string; caption: string }> = {
  live: { name: "Live paper", caption: "trading on live data with paper money; only these results count" },
  ready: { name: "Ready to go live", caption: "a variant passed the backtest gate; not live yet" },
  failing: { name: "Fails the gate", caption: "backtested on the current code; no variant passes the gate yet" },
  untested: { name: "Not screened yet", caption: "no 1y–10y backtest of the current code with standard costs" },
};

export const BENCHMARK = "SPY";

export type Entry = { key: string; bot: Bot; live: Live | undefined; group: Group };

function groupOf(bot: Bot): Group {
  if (bot.stage === "screened") {
    return "ready";
  }
  return bot.backtest?.preset ? "failing" : "untested";
}

export function entriesOf(bots: Bot[]): Entry[] {
  return bots.flatMap((bot): Entry[] =>
    bot.live.length > 0
      ? bot.live.map((live) => ({ key: `run-${live.run.id}`, bot, live, group: "live" }))
      : [{ key: `bot-${bot.name}`, bot, live: undefined, group: groupOf(bot) }],
  );
}

export function excessCagr(glance: Glance | null | undefined): number | undefined {
  const own = glance?.metrics.cagr;
  const spy = glance?.benchmark.cagr;
  return own === null || own === undefined || spy === null || spy === undefined ? undefined : own - spy;
}

export function headline(entry: Entry): number | undefined {
  if (entry.live) {
    const stats = entry.live.run.live_stats;
    return stats && stats.sessions > 0 ? stats.excess_cagr : undefined;
  }
  return excessCagr(entry.bot.backtest);
}

export function windowOf(glance: Glance): string {
  return glance.preset ? `${glance.preset} backtest` : `${glance.period.from.slice(0, 4)}–${glance.period.to.slice(0, 4)} backtest`;
}

export function variantOf(params: Record<string, unknown>, defaults: Record<string, unknown>): string {
  return paramsText(Object.fromEntries(Object.entries(params).filter(([name, value]) => JSON.stringify(value) !== JSON.stringify(defaults[name]))));
}

export function closeness(glance: Glance | null | undefined): number {
  return glance ? glance.failed_checks.length * 100 - (excessCagr(glance) ?? -1) : Infinity;
}
