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

export type PaneTitle = { plugin: ITextWatermarkPluginApi<Time>; text: string; pane: IPaneApi<Time>; bottom: number };

export const moneyFormat = { type: "custom", formatter: money, tickmarksFormatter: moneyTickmarks, minMove: 0.01 } as const;

const TITLE_BAND_PX = 22;
const TITLE_LINE = { fontSize: 11, lineHeight: TITLE_BAND_PX } as const;

export function paneTitle(pane: IPaneApi<Time>, text: string, bottom: number): PaneTitle {
  return { plugin: createTextWatermark(pane, { horzAlign: "left", vertAlign: "top", lines: [{ text, ...TITLE_LINE }] }), text, pane, bottom };
}

export function themeTitles(titles: PaneTitle[]) {
  const color = themeColor("--text-2");
  for (const title of titles) {
    title.plugin.applyOptions({ lines: [{ text: title.text, color, fontFamily: themeFont(), ...TITLE_LINE }] });
  }
}

export function clearTitles(titles: PaneTitle[]) {
  for (const title of titles) {
    const height = title.pane.getHeight();
    if (height > TITLE_BAND_PX * 2) {
      title.pane.priceScale("right").applyOptions({ scaleMargins: { top: TITLE_BAND_PX / height, bottom: title.bottom } });
    }
  }
}

const PAN_SHARE = 0.1;
const ZOOM_STEP = 0.8;

export const USER_INPUTS = ["wheel", "pointerdown", "touchstart", "keydown"] as const;

export const KEY_HINT =
  "Drag across the chart to zoom into that range, shift-drag to pan, scroll or pinch to zoom, drag an axis to rescale it, and double-click to show everything. Arrow keys pan, plus and minus zoom, 0 shows everything.";

export function chartOptions(timeFormatter: (time: Time) => string): DeepPartial<ChartOptions> {
  return {
    autoSize: true,
    layout: { attributionLogo: false, panes: { enableResize: false } },
    handleScroll: { mouseWheel: true, pressedMouseMove: false, horzTouchDrag: true, vertTouchDrag: false },
    handleScale: { mouseWheel: true, pinch: true, axisPressedMouseMove: { time: true, price: true }, axisDoubleClickReset: { time: true, price: true } },
    kineticScroll: { touch: true, mouse: false },
    rightPriceScale: { borderVisible: false, minimumWidth: 64, scaleMargins: { top: 0.08, bottom: 0.06 } },
    timeScale: { borderVisible: false, enableConflation: true, minBarSpacing: 0.01, rightOffsetPixels: 12 },
    localization: { timeFormatter, percentageFormatter: change, tickmarksPercentageFormatter: changeTickmarks },
  };
}

export function followSize(api: IChartApi, element: HTMLElement, onWidth: () => void, onHeight: () => void): () => void {
  let width = api.timeScale().width();
  let height = element.clientHeight;
  const onPlot = () => {
    if (api.timeScale().width() !== width) {
      width = api.timeScale().width();
      requestAnimationFrame(onWidth);
    }
  };
  const observer = new ResizeObserver(() => {
    if (element.clientHeight !== height) {
      height = element.clientHeight;
      requestAnimationFrame(onHeight);
    }
  });
  api.timeScale().subscribeSizeChange(onPlot);
  observer.observe(element);
  requestAnimationFrame(onHeight);
  return () => {
    api.timeScale().unsubscribeSizeChange(onPlot);
    observer.disconnect();
  };
}

export function themedOptions(): DeepPartial<ChartOptions> {
  const crosshair = { color: themeColor("--muted", 0.6), labelBackgroundColor: themeColor("--text-2") };
  return {
    layout: {
      background: { type: ColorType.Solid, color: "transparent" },
      textColor: themeColor("--muted"),
      fontFamily: themeFont(),
      fontSize: 11,
      panes: { separatorColor: themeColor("--border-strong") },
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
