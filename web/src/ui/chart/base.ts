import {
  ColorType,
  type ChartOptions,
  createTextWatermark,
  type DeepPartial,
  type IChartApi,
  type IPaneApi,
  type ITextWatermarkPluginApi,
  type LogicalRange,
  type Time,
} from "lightweight-charts";
import { themeColor, themeFont } from "../../lib/theme";
import { change, changeTickmarks, money, moneyTickmarks } from "./format";

export type PaneTitle = { plugin: ITextWatermarkPluginApi<Time>; text: string };

export const moneyFormat = { type: "custom", formatter: money, tickmarksFormatter: moneyTickmarks, minMove: 0.01 } as const;

export function paneTitle(pane: IPaneApi<Time>, text: string): PaneTitle {
  return { plugin: createTextWatermark(pane, { horzAlign: "left", vertAlign: "top", lines: [{ text, fontSize: 11, lineHeight: 24 }] }), text };
}

export function themeTitles(titles: PaneTitle[]) {
  const color = themeColor("--muted");
  for (const title of titles) {
    title.plugin.applyOptions({ lines: [{ text: title.text, color, fontFamily: themeFont(), fontSize: 11, lineHeight: 24 }] });
  }
}

const PAN_SHARE = 0.1;
const ZOOM_STEP = 0.8;

export const USER_INPUTS = ["wheel", "pointerdown", "touchstart", "keydown"] as const;

export const KEY_HINT = "Arrow keys pan, plus and minus zoom, 0 shows everything. Drag to pan, scroll or pinch to zoom, drag an axis to rescale it and double-click to reset.";

const reducedMotion = () => window.matchMedia("(prefers-reduced-motion: reduce)");

export function chartOptions(timeFormatter: (time: Time) => string): DeepPartial<ChartOptions> {
  return {
    autoSize: true,
    layout: { attributionLogo: false, panes: { enableResize: true } },
    handleScroll: { mouseWheel: true, pressedMouseMove: true, horzTouchDrag: true, vertTouchDrag: false },
    handleScale: { mouseWheel: true, pinch: true, axisPressedMouseMove: { time: true, price: true }, axisDoubleClickReset: { time: true, price: true } },
    kineticScroll: { touch: true, mouse: !reducedMotion().matches },
    rightPriceScale: { borderVisible: false, scaleMargins: { top: 0.22, bottom: 0.06 } },
    timeScale: { borderVisible: false, enableConflation: true, minBarSpacing: 0.01, rightOffset: 2 },
    localization: { timeFormatter, percentageFormatter: change, tickmarksPercentageFormatter: changeTickmarks },
  };
}

export function followReducedMotion(api: IChartApi): () => void {
  const motion = reducedMotion();
  const onMotion = () => api.applyOptions({ kineticScroll: { mouse: !motion.matches } });
  motion.addEventListener("change", onMotion);
  return () => motion.removeEventListener("change", onMotion);
}

export function followWidth(element: HTMLElement, onWidth: () => void): () => void {
  let width = element.clientWidth;
  const observer = new ResizeObserver(() => {
    if (element.clientWidth !== width) {
      width = element.clientWidth;
      requestAnimationFrame(onWidth);
    }
  });
  observer.observe(element);
  return () => observer.disconnect();
}

export function themedOptions(): DeepPartial<ChartOptions> {
  const crosshair = { color: themeColor("--muted", 0.6), labelBackgroundColor: themeColor("--text-2") };
  return {
    layout: {
      background: { type: ColorType.Solid, color: "transparent" },
      textColor: themeColor("--muted"),
      fontFamily: themeFont(),
      fontSize: 11,
      panes: { separatorColor: themeColor("--border"), separatorHoverColor: themeColor("--border-strong") },
    },
    grid: { vertLines: { color: themeColor("--grid") }, horzLines: { color: themeColor("--grid") } },
    crosshair: { vertLine: crosshair, horzLine: crosshair },
  };
}

export function panZoom(key: string, range: LogicalRange): { from: number; to: number } | undefined {
  const width = range.to - range.from;
  const middle = (range.from + range.to) / 2;
  switch (key) {
    case "ArrowLeft":
      return { from: range.from - width * PAN_SHARE, to: range.to - width * PAN_SHARE };
    case "ArrowRight":
      return { from: range.from + width * PAN_SHARE, to: range.to + width * PAN_SHARE };
    case "+":
    case "=":
      return { from: middle - (width * ZOOM_STEP) / 2, to: middle + (width * ZOOM_STEP) / 2 };
    case "-":
      return { from: middle - width / ZOOM_STEP / 2, to: middle + width / ZOOM_STEP / 2 };
    default:
      return undefined;
  }
}
