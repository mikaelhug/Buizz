import { api, must, type Schemas } from "../api/client";
import { useResource } from "../lib/useResource";

export function PresetDates({ preset, className }: { preset: Schemas["PeriodPreset"]; className: string }) {
  const periods = useResource(() => must(api.GET("/api/periods")), []);
  const described = periods.data?.find((entry) => entry.preset === preset);
  if (!described) {
    return null;
  }
  const { from, to } = described.period;
  return (
    <p className={className} role="status">
      {from} → {to}
      {described.shortened && ", shortened to start where Alpaca's daily history begins"}
    </p>
  );
}
