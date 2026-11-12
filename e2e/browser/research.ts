import type { Page } from "playwright-core";
import { pill, signedIn } from "./session.ts";

async function twoVariantsLive(page: Page): Promise<{ params: string[] }> {
  await page.getByRole("link", { name: "sma_cross", exact: true }).click();
  await page.getByRole("navigation", { name: "Bot views" }).getByRole("link", { name: "Research" }).click();
  await page.locator(".research-trials", { hasText: "4 variants tried" }).waitFor();
  await page.getByRole("region", { name: "Heatmap" }).locator("table").waitFor();
  const variants = page.getByRole("region", { name: "Variants" }).locator("tbody tr");
  if ((await variants.count()) !== 4) {
    throw new Error(`the research table shows ${await variants.count()} variants, not 4`);
  }

  for (let started = 0; started < 2; started++) {
    const research = page.url();
    await variants.getByRole("button", { name: /^Go live with / }).first().click();
    await page.waitForFunction((before) => location.href !== before && /\/runs\/\d+$/.test(location.hash), research);
    await page.goBack();
    await variants.locator(".pill-live").nth(started).waitFor();
  }

  await page.getByRole("link", { name: "Bots", exact: true }).click();
  const live = page.getByRole("region", { name: "Live", exact: true }).locator("tbody tr").filter({ has: page.getByRole("link", { name: "sma_cross", exact: true }) });
  await live.nth(1).waitFor();
  const params = await live.locator(".variant-params").allTextContents();
  await live.first().getByRole("button", { name: "Pause", exact: true }).click();
  await pill(live.first(), "paused").waitFor();
  await live.nth(1).getByRole("button", { name: "Pause", exact: true }).waitFor();
  return { params };
}

await signedIn(twoVariantsLive);
