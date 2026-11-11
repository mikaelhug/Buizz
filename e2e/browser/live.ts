import type { Page } from "playwright-core";
import { env, panel, pill, signedIn } from "./session.ts";

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

function shown(at: Date): string {
  return at.toISOString().slice(0, 16).replace("T", " ");
}

function minutes({ from, through }: Released): string[] {
  const stamps: string[] = [];
  for (let at = Date.parse(from); at <= Date.parse(through); at += MINUTE_MS) {
    stamps.push(new Date(at).toISOString().replace(".000Z", "Z"));
  }
  return stamps;
}

async function pauseAndResume(page: Page): Promise<{ paused: Released; resumed: Released }> {
  const card = (name: string) => page.locator("article.bot-card").filter({ has: page.getByRole("link", { name, exact: true }) });
  for (const name of ["tape_twin", "tape"]) {
    await card(name).getByRole("button", { name: "Live" }).click();
    await pill(card(name), "live").waitFor();
  }
  await card("tape").getByRole("link", { name: "tape", exact: true }).click();

  const head = page.locator(".detail-head");
  const run = page.locator(".run-head");
  const orders = panel(page, "Orders");
  const events = panel(page, "Events");
  const resting = orders.locator("tr", { hasText: "resting bid" });
  await pill(orders, "filled").first().waitFor();
  await pill(resting, "working").waitFor();

  await head.getByRole("button", { name: "Pause" }).click();
  await pill(head, "paused").waitFor();
  await pill(run, "paused").waitFor();
  await pill(resting, "cancelled").waitFor();

  const paused = await release(4);
  const markedThrough = shown(new Date(Date.parse(paused.through) + MINUTE_MS));
  await run.locator(".facts div", { hasText: "Clock" }).locator("dd", { hasText: markedThrough }).waitFor();

  await head.getByRole("button", { name: "Live" }).click();
  await pill(head, "live").waitFor();
  await pill(run, "running").waitFor();
  const resumed = await release(60);
  await events.locator(".event-text", { hasText: `@${resumed.from}` }).first().waitFor();

  const logged = (await events.locator(".event-text").allTextContents()).join("\n");
  const leaked = minutes(paused).filter((minute) => logged.includes(`@${minute}`));
  if (leaked.length > 0) {
    throw new Error(`the bot saw bars from its paused interval: ${leaked.join(", ")}`);
  }
  return { paused, resumed };
}

await signedIn(pauseAndResume);
