import type { Page } from "playwright-core";
import { env, panel, pill, signedIn } from "./session.ts";

type Seen = { run: number; months: Record<string, string>; benchmarkReturn: string; trades: number };

async function customPeriodBacktest(page: Page): Promise<Seen> {
  await page.getByRole("link", { name: "red_green", exact: true }).click();
  await page.locator(".detail-head").getByRole("button", { name: "Backtest" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("radio", { name: "custom" }).click();
  await dialog.getByLabel("From").fill(env("PERIOD_FROM"));
  await dialog.getByLabel("To", { exact: true }).fill(env("PERIOD_TO"));
  await dialog.getByLabel("Drawdown stop %").fill("90");
  const shown = page.url();
  await dialog.getByRole("button", { name: "Run", exact: true }).click();

  await page.waitForFunction((before) => location.href !== before, shown);
  const run = Number(/\/runs\/(\d+)$/.exec(page.url())?.[1]);
  if (!Number.isInteger(run)) {
    throw new Error(`the page names no run: ${page.url()}`);
  }
  const head = page.locator(".run-head").filter({ has: page.getByRole("heading", { name: `Run #${run}`, exact: true }) });
  await pill(head, "completed").waitFor();

  const monthly = panel(page, "Monthly returns").locator("table");
  const year = monthly.locator("tbody tr").first();
  await year.waitFor();
  const headings = await monthly.locator("thead th").allTextContents();
  const cells = await year.locator("th, td").allTextContents();
  const months = Object.fromEntries(headings.map((heading, index) => [heading, cells[index] ?? ""]).filter(([heading, cell]) => heading && cell));

  const benchmarkReturn = await page.locator(".metric", { hasText: "Return" }).first().locator(".metric-bench").textContent();
  const trades = panel(page, "Trades");
  await trades.locator(".metric", { hasText: "Net P&L" }).waitFor();
  return { run, months, benchmarkReturn: benchmarkReturn ?? "", trades: await trades.locator("tbody tr").count() };
}

await signedIn(customPeriodBacktest);
