import { fmt, tone } from "../lib/format";

export const SUB_PERIODS_ABOUT =
  "The period split into three equal thirds. Each cell is the return above total-return SPY in that third, at SPY's volatility: green where the variant came out ahead. An edge that holds in every third is more likely real than one earned in a single stretch.";

export function SubPeriods({ edges, count = true }: { edges: number[]; count?: boolean }) {
  if (edges.length === 0) {
    return <span className="muted">—</span>;
  }
  const ahead = edges.filter((edge) => edge > 0).length;
  const summary = `Ahead of total-return SPY in ${ahead} of ${edges.length} thirds: ${edges.map((edge) => fmt.change(edge)).join(", ")}`;
  return (
    <span className="sub-periods" role="img" aria-label={summary} title={summary}>
      {edges.map((edge, index) => (
        <span key={index} className={`sub-period ${tone(edge) ?? ""}`} />
      ))}
      {count && (
        <span className="sub-periods-count num">
          {ahead}/{edges.length}
        </span>
      )}
    </span>
  );
}
