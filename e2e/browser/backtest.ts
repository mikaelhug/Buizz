import type { Page } from "playwright-core";
import { env, pill, signedIn } from "./session.ts";

async function customPeriodBacktest(page: Page): Promise<{ run: number }> {
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
  return { run };

}

await signedIn(customPeriodBacktest);
