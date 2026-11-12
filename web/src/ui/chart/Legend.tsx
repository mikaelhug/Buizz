import { ABSENT, fmt, tone } from "../../lib/format";
import { money, wallDate } from "./format";
import type { Mark } from "./model";

export type Reading = { value?: number | undefined; change?: number | undefined; drawdown?: number | undefined };

export type Readout = {
  time?: number | undefined;
  intraday: boolean;
  strategy: Reading;
  benchmark: Reading;
  floor: Reading;
  exposure?: number | undefined;
  marks: Mark[];
};

export type SeriesKey = "strategy" | "benchmark" | "floor";

type Props = {
  readout: Readout;
  names: Record<SeriesKey, string>;
  hidden: Record<SeriesKey, boolean>;
  exposure: boolean;
  envelope: boolean;
  onToggle: (key: SeriesKey) => void;
};

const SERIES: SeriesKey[] = ["strategy", "benchmark"];

const ENVELOPE_ABOUT =
  "The benchmark plus the source backtest's 5th-percentile excess return over every window as many sessions long as the run was at that point. From session 20 an excess return below it falsifies the run";

function Toggle({ name, swatch, hidden, onToggle }: { name: string; swatch: SeriesKey; hidden: boolean; onToggle: () => void }) {
  return (
    <button type="button" className="legend-toggle" aria-pressed={!hidden} title={hidden ? `Show ${name}` : `Hide ${name}`} onClick={onToggle}>
      <span className={`legend-swatch legend-swatch-${swatch}`} aria-hidden="true" />
      <span className="legend-name">{name}</span>
    </button>
  );
}

export function Legend({ readout, names, hidden, exposure, envelope, onToggle }: Props) {
  return (
    <div className="chart-legend">
      <time className="legend-time">{readout.time === undefined ? "" : wallDate(readout.time, readout.intraday)}</time>
      {SERIES.map((key) => {
        const reading = readout[key];
        return (
          <div key={key} className={hidden[key] ? "legend-row legend-off" : "legend-row"}>
            <Toggle name={names[key]} swatch={key} hidden={hidden[key]} onToggle={() => onToggle(key)} />
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
      {envelope && (
        <div className={hidden.floor ? "legend-row legend-off" : "legend-row"}>
          <Toggle name={names.floor} swatch="floor" hidden={hidden.floor} onToggle={() => onToggle("floor")} />
          <span className="num legend-value">{readout.floor.value === undefined ? ABSENT : money(readout.floor.value)}</span>
          <span className="legend-key" title={ENVELOPE_ABOUT}>
            5th percentile
          </span>
          <span />
        </div>
      )}
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
