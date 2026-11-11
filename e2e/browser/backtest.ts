import type { Page } from "playwright-core";
import { env, pill, signedIn } from "./session.ts";

async function customPeriodBacktest(page: Page): Promise<{ run: number }> {
  await page.getByRole("link", { name: "red_green", exact: true }).click();
  await page.locator(".detail-head").getByRole("button", { name: "Backtest" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("radio", { name: "custom" }).click();
  await dialog.getByLabel("From").fill(env("PERIOD_FROM"));
  await dialog.getByLabel("To").fill(env("PERIOD_TO"));
  await dialog.getByRole("button", { name: "Run", exact: true }).click();

  const head = page.locator(".run-head");
  await pill(head, "completed").waitFor();
  const heading = await head.getByRole("heading").textContent();
  const run = Number(heading?.replace(/\D/g, ""));
  if (!Number.isInteger(run)) {
    throw new Error(`the run heading names no run: ${heading}`);
  }
  return { run };
}

await signedIn(customPeriodBacktest);
