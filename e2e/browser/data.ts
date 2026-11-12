import type { Page } from "playwright-core";
import { panel, pill, signedIn } from "./session.ts";

async function dataHealth(page: Page): Promise<{ warnings: string[] }> {
  await page.getByRole("link", { name: "Data", exact: true }).click();
  const jobs = panel(page, "Jobs");
  await pill(jobs.locator("li", { hasText: "corporate_actions" }), "backoff").waitFor();
  await pill(jobs.locator("li", { hasText: "tiingo_history" }), "failed").waitFor();
  const warnings = panel(page, "Warnings").locator("li");
  await warnings.filter({ hasText: "overlap rejected" }).filter({ hasText: "AAPL" }).waitFor();
  await panel(page, "Backfill").locator("li", { hasText: "Bars1d" }).first().waitFor();
  await panel(page, "Coverage").locator("tr", { hasText: "AAPL" }).first().waitFor();
  return { warnings: await warnings.allTextContents() };
}

await signedIn(dataHealth);
