import type { Page } from "playwright-core";
import { env, openTab, panel, pill, signedIn } from "./session.ts";

type Released = { from: string; through: string };

const MINUTE_MS = 60_000;

async function release(minutes: number): Promise<Released> {
  const response = await fetch(`${env("FAKE_URL")}/control/release`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ minutes }),
  });
  if (!response.ok) {
    throw new Error(`the fake refused to release bars: ${response.status}`);
  }
  return (await response.json()) as Released;
}

const exchangeTime = new Intl.DateTimeFormat("sv-SE", {
  timeZone: "America/New_York",
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
});

function shown(at: Date): string {
  return `${exchangeTime.format(at)} ET`;
}

function minutes({ from, through }: Released): string[] {
  const stamps: string[] = [];
  for (let at = Date.parse(from); at <= Date.parse(through); at += MINUTE_MS) {
    stamps.push(new Date(at).toISOString().replace(".000Z", "Z"));
  }
  return stamps;
}

async function pauseAndResumeThenStop(page: Page): Promise<{ paused: Released; resumed: Released }> {
  const list = page.getByRole("region", { name: "Bot list", exact: true });
  const row = (name: string) => list.locator("tbody tr").filter({ has: page.getByRole("link", { name, exact: true }) });
  for (const name of ["tape_twin", "tape"]) {
    await row(name).getByRole("button", { name: `Actions for ${name}`, exact: true }).click();
    await page.getByRole("menuitem", { name: "Go live", exact: true }).click();
    await row(name).locator(".pill-incubating").waitFor();
  }
  const live = page.getByRole("region", { name: "Live paper", exact: true });
  await live.getByRole("link", { name: "tape", exact: true }).waitFor();
  const header = await list.locator("thead").textContent();
  for (const column of ["CAGR vs SPY", "Status", "Backtest", "Equity vs SPY"]) {
    if (!header?.includes(column)) {
      throw new Error(`the bot list has no ${column} column: ${header}`);
    }
  }
  await row("tape").getByRole("link", { name: "tape", exact: true }).click();

  const head = page.locator(".detail-head");
  const run = page.locator(".run-bar");
  const orders = panel(page, "Orders");
  const events = panel(page, "Events");
  const resting = orders.locator("tr", { hasText: "resting bid" });
  await openTab(page, "Orders");
  await pill(orders, "filled").first().waitFor();
  await pill(resting, "working").waitFor();

  await head.getByRole("button", { name: "Pause", exact: true }).click();
  await pill(head, "paused").waitFor();
  await pill(run, "paused").waitFor();
  await pill(resting, "cancelled").waitFor();

  const paused = await release(4);
  const markedThrough = shown(new Date(Date.parse(paused.through) + MINUTE_MS));
  await run.locator(".facts div", { hasText: "Clock" }).locator("dd", { hasText: markedThrough }).waitFor();

  await head.getByRole("button", { name: "Live", exact: true }).click();
  await pill(head, "live").waitFor();
  await pill(run, "running").waitFor();
  const resumed = await release(60);
  await openTab(page, "Positions & events");
  await events.locator(".event-text", { hasText: `@${resumed.from}` }).first().waitFor();

  const logged = (await events.locator(".event-text").allTextContents()).join("\n");
  const leaked = minutes(paused).filter((minute) => logged.includes(`@${minute}`));
  if (leaked.length > 0) {
    throw new Error(`the bot saw bars from its paused interval: ${leaked.join(", ")}`);
  }

  await head.getByRole("button", { name: "Stop", exact: true }).click();
  await page.getByRole("dialog").getByRole("button", { name: "Stop run" }).click();
  await pill(run, "cancelled").waitFor();
  await head.getByRole("button", { name: "Go live", exact: true }).waitFor();
  return { paused, resumed };
}

await signedIn(pauseAndResumeThenStop);
