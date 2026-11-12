import { ABSENT, fmt, tone } from "../../lib/format";
import { money, wallDate } from "./format";
import type { Mark } from "./model";

export type Reading = { value?: number | undefined; change?: number | undefined; drawdown?: number | undefined };

export type Readout = {
  time?: number | undefined;
  intraday: boolean;
  strategy: Reading;
  benchmark: Reading;
  exposure?: number | undefined;
  marks: Mark[];
};

export type SeriesKey = "strategy" | "benchmark";

type Props = {
  readout: Readout;
  names: Record<SeriesKey, string>;
  hidden: Record<SeriesKey, boolean>;
  exposure: boolean;
  onToggle: (key: SeriesKey) => void;
};

const SERIES: SeriesKey[] = ["strategy", "benchmark"];

export function Legend({ readout, names, hidden, exposure, onToggle }: Props) {
  return (
    <div className="chart-legend">
      <time className="legend-time">{readout.time === undefined ? "" : wallDate(readout.time, readout.intraday)}</time>
      {SERIES.map((key) => {
        const reading = readout[key];
        return (
          <div key={key} className={hidden[key] ? "legend-row legend-off" : "legend-row"}>
            <button type="button" className="legend-toggle" aria-pressed={!hidden[key]} title={hidden[key] ? `Show ${names[key]}` : `Hide ${names[key]}`} onClick={() => onToggle(key)}>
              <span className={`legend-swatch legend-swatch-${key}`} aria-hidden="true" />
              <span className="legend-name">{names[key]}</span>
            </button>
            <span className="num legend-value">{reading.value === undefined ? ABSENT : money(reading.value)}</span>
            <span className={`num legend-change ${tone(reading.change) ?? ""}`} title="Return since the start of the visible range">
              {fmt.change(reading.change)}
            </span>
            <span className="num legend-drawdown" title="Drawdown from the running peak">
              <span className="legend-key">DD</span> {fmt.percent(reading.drawdown)}
            </span>
          </div>
        );
      })}
      {exposure && readout.exposure !== undefined && (
        <div className="legend-row legend-note">
          Gross exposure <span className="num">{fmt.percent(readout.exposure)}</span>
        </div>
      )}
      {readout.marks.map((mark) => (
        <div key={mark.id} className={`legend-row legend-note legend-mark-${mark.kind}`}>
          {mark.text}
        </div>
      ))}
    </div>
  );
}
