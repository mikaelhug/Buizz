import type { IChartApi, LogicalRange, UTCTimestamp } from "lightweight-charts";

const MIN_BRUSH_PX = 6;
const EDGE_PX = 12;

type Drag = { pan: boolean; x: number; range: LogicalRange };

export function fitPrices(api: IChartApi) {
  for (const pane of api.panes()) {
    pane.priceScale("right").applyOptions({ autoScale: true });
  }
}

export function showSpan(api: IChartApi, from: number, to: number) {
  const timeScale = api.timeScale();
  const first = timeScale.timeToIndex(from as UTCTimestamp, true);
  const last = timeScale.timeToIndex(to as UTCTimestamp, true);
  const width = timeScale.width();
  if (first === null || last === null || width <= EDGE_PX * 2) {
    return;
  }
  if (last <= first) {
    timeScale.setVisibleLogicalRange({ from: first - 1, to: first + 1 });
    return;
  }
  const pad = (EDGE_PX * (last - first)) / (width - EDGE_PX * 2);
  timeScale.setVisibleLogicalRange({ from: first + 0.5 - pad, to: last - 0.5 + pad });
}

export function brushZoom(api: IChartApi, host: HTMLElement): () => void {
  const band = document.createElement("div");
  band.className = "chart-brush";
  band.hidden = true;
  band.setAttribute("aria-hidden", "true");
  host.append(band);
  let drag: Drag | undefined;

  const timeScale = api.timeScale();
  const plotHeight = () => host.clientHeight - timeScale.height();
  const offsetX = (event: PointerEvent) => event.clientX - host.getBoundingClientRect().left;
  const clampX = (x: number) => Math.min(Math.max(x, 0), timeScale.width());

  const finish = () => {
    drag = undefined;
    band.hidden = true;
    host.classList.remove("chart-panning");
    window.removeEventListener("pointermove", onMove);
    window.removeEventListener("pointerup", onUp);
    window.removeEventListener("pointercancel", finish);
    window.removeEventListener("keydown", onKey);
  };

  const onMove = (event: PointerEvent) => {
    if (!drag) {
      return;
    }
    const x = clampX(offsetX(event));
    if (drag.pan) {
      const shift = (drag.x - x) / timeScale.options().barSpacing;
      timeScale.setVisibleLogicalRange({ from: drag.range.from + shift, to: drag.range.to + shift });
      return;
    }
    band.hidden = Math.abs(x - drag.x) < MIN_BRUSH_PX;
    band.style.left = `${Math.min(x, drag.x)}px`;
    band.style.width = `${Math.abs(x - drag.x)}px`;
    band.style.height = `${plotHeight()}px`;
  };

  const onUp = (event: PointerEvent) => {
    const started = drag;
    finish();
    if (!started || started.pan) {
      return;
    }
    const x = clampX(offsetX(event));
    const from = timeScale.coordinateToLogical(Math.min(x, started.x));
    const to = timeScale.coordinateToLogical(Math.max(x, started.x));
    if (Math.abs(x - started.x) >= MIN_BRUSH_PX && from !== null && to !== null) {
      fitPrices(api);
      const edge = to - from > 1 ? 0.5 : 0;
      timeScale.setVisibleLogicalRange({ from: from + edge, to: to - edge });
    }
  };

  const onKey = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      finish();
    }
  };

  const onDown = (event: PointerEvent) => {
    const x = offsetX(event);
    const y = event.clientY - host.getBoundingClientRect().top;
    const range = timeScale.getVisibleLogicalRange();
    if (event.pointerType !== "mouse" || event.button !== 0 || !range || x > timeScale.width() || y > plotHeight()) {
      return;
    }
    drag = { pan: event.shiftKey, x, range };
    host.classList.toggle("chart-panning", drag.pan);
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", finish);
    window.addEventListener("keydown", onKey);
  };

  host.addEventListener("pointerdown", onDown);
  return () => {
    finish();
    host.removeEventListener("pointerdown", onDown);
    band.remove();
  };
}
