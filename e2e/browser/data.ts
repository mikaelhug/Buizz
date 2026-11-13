import type { Page } from "playwright-core";
import { panel, pill, signedIn } from "./session.ts";

async function dataHealth(page: Page): Promise<{ warnings: string[] }> {
  await page.getByRole("link", { name: "Data", exact: true }).click();
  await page.getByRole("region", { name: "Spine health" }).waitFor();
  await page.getByRole("group", { name: "Data view" }).getByRole("button", { name: "Details" }).click();
  const jobs = panel(page, "Jobs");
  await pill(jobs.getByRole("row", { name: /corporate_actions/ }), "backoff").waitFor();
  await pill(jobs.getByRole("row", { name: /tiingo_history/ }), "failed").waitFor();
  const warnings = panel(page, "Warnings").locator("tbody").getByRole("row");
  await warnings.filter({ hasText: "overlap rejected" }).filter({ hasText: "AAPL" }).waitFor();
  await panel(page, "Backfill").locator("li", { hasText: "Bars1d" }).first().waitFor();
  await panel(page, "Coverage").getByRole("row", { name: /AAPL/ }).first().waitFor();
  return { warnings: await warnings.allTextContents() };
}

await signedIn(dataHealth);
