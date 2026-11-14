import type { Page } from "playwright-core";
import { env, openTab, panel, pill, signedIn } from "./session.ts";

type Seen = { run: number; months: Record<string, string>; benchmarkReturn: string; positions: string; trades: number; chart: string };

async function zoomThenReset(page: Page): Promise<void> {
  const equity = page.getByRole("region", { name: "Equity", exact: true });
  const plot = equity.locator(".equity-chart");
  const reset = equity.getByRole("button", { name: "Reset", exact: true });
  await plot.scrollIntoViewIfNeeded();
  const box = await plot.boundingBox();
  if (!box) {
    throw new Error("the equity chart is not on screen");
  }
  const y = box.y + box.height / 3;
  await page.mouse.move(box.x + box.width * 0.3, y);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width * 0.6, y, { steps: 8 });
  await equity.locator(".chart-brush:not([hidden])").waitFor();
  await page.mouse.up();
  await reset.waitFor();
  await plot.dblclick({ position: { x: box.width / 2, y: box.height / 3 } });
  await reset.waitFor({ state: "detached" });
}

async function customPeriodBacktest(page: Page): Promise<Seen> {
  await page.getByRole("link", { name: "red_green", exact: true }).click();
  await page.locator(".detail-head").getByRole("button", { name: "Backtest" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("radio", { name: "custom" }).click();
  await dialog.getByLabel("From").fill(env("PERIOD_FROM"));
  await dialog.getByLabel("To", { exact: true }).fill(env("PERIOD_TO"));
  await dialog.getByLabel("Drawdown stop %", { exact: true }).fill("90");
  const shown = page.url();
  await dialog.getByRole("button", { name: "Run", exact: true }).click();

  await page.waitForFunction((before) => location.href !== before, shown);
  const run = Number(/\/runs\/(\d+)$/.exec(page.url())?.[1]);
  if (!Number.isInteger(run)) {
    throw new Error(`the page names no run: ${page.url()}`);
  }
  const head = page.locator(".run-bar").filter({ has: page.getByRole("heading", { name: `Run #${run}`, exact: true }) });
  await pill(head, "completed").waitFor();
  await zoomThenReset(page);

  const monthly = panel(page, "Monthly returns").locator("table");
  const year = monthly.locator("tbody tr").first();
  await year.waitFor();
  const headings = await monthly.locator("thead th").allTextContents();
  const cells = await year.locator("th, td").allTextContents();
  const months = Object.fromEntries(headings.map((heading, index) => [heading, cells[index] ?? ""]).filter(([heading, cell]) => heading && cell));

  const benchmarkReturn = await page.getByRole("region", { name: "Returns", exact: true }).locator(".metric-headline .metric-bench").textContent();
  await panel(page, "Behaviour").locator("thead .cell-sub", { hasText: `→ ${env("PERIOD_TO")}` }).first().waitFor();
  const positions = panel(page, "Behaviour").locator("tbody tr").filter({ has: page.getByRole("rowheader", { name: "Positions" }) }).locator("td").first();
  await positions.waitFor();
  const held = (await positions.textContent()) ?? "";
  await openTab(page, "Trades");
  const trades = panel(page, "Trades");
  await trades.locator(".metric", { hasText: "Net P&L" }).waitFor();
  const tradeCount = await trades.locator("tbody tr").count();
  await trades.getByRole("link", { name: "AAA", exact: true }).first().click();
  const chart = page.locator("section.price-panel").getByRole("img");
  await page.waitForFunction(() => /fills/.test(document.querySelector("section.price-panel [role=img]")?.getAttribute("aria-label") ?? ""));
  return { run, months, benchmarkReturn: benchmarkReturn ?? "", positions: held, trades: tradeCount, chart: (await chart.getAttribute("aria-label")) ?? "" };
}

await signedIn(customPeriodBacktest);
