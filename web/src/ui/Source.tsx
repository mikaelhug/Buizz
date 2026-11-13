import { fmt, tone } from "../lib/format";
import { href } from "../lib/route";
import { excessCagr, windowOf } from "../lib/bots";
import { GateResult, gateText } from "./Gate";
import type { Live } from "./LiveTable";
import { SubPeriods } from "./SubPeriods";

export function sourceSentence(live: Live): string {
  const source = live.source;
  if (!source) {
    return live.run.source_run ? `Went live from backtest #${live.run.source_run}.` : "Went live with the bot's default params, without a source backtest.";
  }
  const excess = excessCagr(source);
  return `Went live from backtest #${source.run_id} (${windowOf(source)}): ${fmt.change(excess)} a year vs ${source.benchmark_symbol}, ${gateText(source.failed_checks)}.`;
}

export function SourceBacktest({ live }: { live: Live }) {
  const source = live.source;
  const id = source?.run_id ?? live.run.source_run;
  if (id === null || id === undefined) {
    return <span className="muted">no source backtest</span>;
  }
  const link = (
    <a href={href({ page: "bot", name: live.run.bot, run: id })} aria-label={`Source backtest #${id}`}>
      #{id}
    </a>
  );
  if (!source) {
    return link;
  }
  const excess = excessCagr(source);
  return (
    <span className="source">
      <span className="source-what">
        {link}
        <span className={`num ${tone(excess) ?? ""}`}>{fmt.change(excess)}</span>
        <span className="muted">{source.preset ?? "custom"}</span>
      </span>
      <span className="source-checks">
        <GateResult checks={source.failed_checks} run={source.run_id} />
        <SubPeriods edges={source.sub_periods} count={false} />
      </span>
    </span>
  );
}
