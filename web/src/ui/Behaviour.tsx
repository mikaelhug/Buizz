import { api, must, type Schemas } from "../api/client";
import { useSpineEvents } from "../api/stream";
import { ABSENT, fmt } from "../lib/format";
import { useResource } from "../lib/useResource";
import { StaleFailure } from "./Feedback";
import { Info } from "./Popover";

type Fingerprint = Schemas["Fingerprint"];

type Row = {
  name: string;
  title: string;
  value: (fingerprint: Fingerprint) => string;
};

const months = (value: number | null | undefined) => (value === null || value === undefined ? ABSENT : `${fmt.integer(Math.round(value * 10) / 10)} mo`);
const range = (fingerprint: Fingerprint) =>
  fingerprint.positions_median === null || fingerprint.positions_median === undefined
    ? ABSENT
    : `${fmt.integer(fingerprint.positions_median)} (${fmt.integer(fingerprint.positions_min)}–${fmt.integer(fingerprint.positions_max)})`;

const ROWS: Row[] = [
  { name: "Positions", title: "Stocks held at each observation (session close, or quarter end for a 13F reference): median, with the fewest and most. ETFs such as a T-bill fund are not stocks.", value: range },
  { name: "Top-5 share", title: "Weight of the five largest stock positions in the stock book, averaged over the observations.", value: (f) => fmt.percent(f.top_five_share) },
  { name: "Turnover", title: "Martin and Puthenpurackal's definition: the smaller of stock purchases and stock sales, over the average stock book, per year.", value: (f) => fmt.percent(f.turnover) },
  { name: "Holding period", title: "Months from the first observation holding a stock to the first without it; positions still open count to the end. Median, then mean, over every holding.", value: (f) => `${months(f.holding_months_median)} · ${months(f.holding_months_mean)}` },
  { name: "Stock share", title: "Stock book over equity, averaged; the rest is cash, T-bills or bonds. A 13F shows no cash, so a reference has none.", value: (f) => fmt.percent(f.stock_share) },
  { name: "Idle months", title: "Share of calendar months with no fill at all. A quarterly 13F cannot show months.", value: (f) => fmt.percent(f.idle_months) },
  { name: "Idle quarters", title: "Share of calendar quarters with no fill at all, or for a 13F no change in any holding.", value: (f) => fmt.percent(f.idle_quarters) },
  { name: "Buys into weakness", title: "Share of stock buy dollars placed when the stock closed at least 20% below its 52-week high, or the benchmark at least 10% below its own.", value: (f) => fmt.percent(f.buys_into_weakness) },
  { name: "Sells on price", title: "Share of stock sell dollars whose order reason starts with “price:”, or that the drawdown stop forced. A 13F carries no reasons.", value: (f) => fmt.percent(f.sells_on_price) },
  { name: "Beta", title: "Value-weighted beta of the stocks held at each quarter end, each from a year of daily returns against the benchmark, averaged over the quarters.", value: (f) => fmt.ratio(f.beta) },
];

export function BehaviourPanel({ run }: { run: Schemas["RunSummary"] }) {
  const behaviour = useResource(() => must(api.GET("/api/runs/{id}/behaviour", { params: { path: { id: run.id } } })), [run.id]);
  useSpineEvents((event) => {
    if ("run_id" in event && event.run_id === run.id && (event.type === "run_status" || event.type === "live_session")) {
      behaviour.reload();
    }
  });
  const own = behaviour.data?.run;
  if (!own) {
    return null;
  }
  const columns = [{ name: `Run #${run.id}`, fingerprint: own }, ...(behaviour.data?.references ?? []).map((reference) => ({ name: reference.name, fingerprint: reference.fingerprint }))];
  const windows = own.drawdowns.map((window, index) => ({ window, values: columns.map((column) => column.fingerprint.drawdowns[index]?.net_buying) }));
  return (
    <section className="panel" aria-label="Behaviour">
      <StaleFailure resource={behaviour} />
      <header className="panel-head">
        <h2>Behaviour</h2>
        <span className="muted">How the run held and traded, beside a reference investor's public record over the same years</span>
      </header>
      <div className="table-wrap">
        <table className="table fin">
          <thead>
            <tr>
              <th scope="col">
                <span className="visually-hidden">Measure</span>
              </th>
              {columns.map((column) => (
                <th key={column.name} scope="col" className="r">
                  {column.name}
                  <span className="muted">
                    {" "}
                    {column.fingerprint.from} → {column.fingerprint.to}
                  </span>
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {ROWS.map((row) => (
              <tr key={row.name}>
                <th scope="row">
                  {row.name}
                  <Info label={row.name}>{row.title}</Info>
                </th>
                {columns.map((column) => (
                  <td key={column.name} className="r num">
                    {row.value(column.fingerprint)}
                  </td>
                ))}
              </tr>
            ))}
            {windows.map(({ window, values }) => (
              <tr key={window.label}>
                <th scope="row">
                  {window.label}
                  <Info label={window.label}>
                    Net stock buying (+) or selling (−) from {window.from} to {window.to}, the benchmark's peak to trough, as a share of equity before it began (of the 13F book for a reference, whose trades are dated by quarter).
                  </Info>
                </th>
                {values.map((value, index) => (
                  <td key={columns[index]?.name} className="r num">
                    {fmt.change(value)}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
}
