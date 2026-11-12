import type { Page } from "playwright-core";
import { pill, signedIn } from "./session.ts";

async function twoVariantsLive(page: Page): Promise<{ params: string[] }> {
  const list = page.getByRole("region", { name: "Bot list", exact: true });
  const screened = list.locator("tbody tr").filter({ has: page.getByRole("link", { name: "sma_cross", exact: true }) });
  await screened.getByText(/^backtest 1y/).waitFor();
  await screened.getByRole("img", { name: /^Since / }).waitFor();
  await page.keyboard.press("ControlOrMeta+k");
  await page.getByRole("combobox", { name: /^Search pages/ }).fill("Research sma_cross");
  await page.getByRole("option", { name: /^Research sma_cross/ }).waitFor();
  await page.keyboard.press("Enter");
  await page.locator(".research-trials", { hasText: "4 variants tried" }).waitFor();
  await page.getByRole("region", { name: "Heatmap" }).locator("table").waitFor();
  const variants = page.getByRole("region", { name: "Variants" }).locator("tbody tr");
  if ((await variants.count()) !== 4) {
    throw new Error(`the research table shows ${await variants.count()} variants, not 4`);
  }

  await variants.nth(0).getByRole("checkbox").check();
  await variants.nth(1).getByRole("checkbox").check();
  await page.getByRole("toolbar", { name: "Selected variants" }).getByRole("link", { name: "Compare 2" }).click();
  const compared = page.getByRole("region", { name: "Metrics compared" }).locator("thead th a");
  await compared.nth(1).waitFor();
  if ((await compared.count()) !== 2) {
    throw new Error(`the comparison shows ${await compared.count()} runs, not 2`);
  }
  await page.goBack();

  for (let started = 0; started < 2; started++) {
    const research = page.url();
    await variants.getByRole("button", { name: /^Go live with / }).first().click();
    await page.waitForFunction((before) => location.href !== before && /\/runs\/\d+$/.test(location.hash), research);
    await page.goBack();
    await variants.locator(".pill-live").nth(started).waitFor();
  }

  await page.getByRole("navigation", { name: "Main" }).getByRole("link", { name: "Bots", exact: true }).click();
  const live = list.locator("tbody tr").filter({ has: page.getByRole("link", { name: "sma_cross", exact: true }) });
  await live.nth(1).waitFor();
  const params = await live.locator(".variant-params").allTextContents();
  await live.first().getByRole("button", { name: /^Actions for sma_cross run #/ }).click();
  await page.getByRole("menuitem", { name: "Pause", exact: true }).click();
  await pill(live.first(), "paused").waitFor();
  if ((await pill(live.nth(1), "paused").count()) !== 0) {
    throw new Error("pausing one variant paused the other");
  }
  return { params };
}

await signedIn(twoVariantsLive);
