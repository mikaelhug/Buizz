import { type Browser, chromium, type Locator, type Page } from "playwright-core";

const TIMEOUT_MS = 60_000;

export function env(name: string): string {
  const value = process.env[name];
  if (!value) {
    throw new Error(`${name} is not set`);
  }
  return value;
}

export function panel(page: Page, heading: string): Locator {
  return page.locator("section.panel").filter({ has: page.getByRole("heading", { name: heading, exact: true }) });
}

export function pill(scope: Locator, status: string): Locator {
  return scope.locator(".pill", { hasText: new RegExp(`^${status}$`) });
}

export async function signedIn(run: (page: Page) => Promise<unknown>): Promise<void> {
  const browser: Browser = await chromium.launch({ headless: true });
  try {
    const context = await browser.newContext({ baseURL: env("BUIZZ_URL"), timezoneId: "UTC" });
    context.setDefaultTimeout(TIMEOUT_MS);
    const page = await context.newPage();
    await page.goto("/");
    await page.getByLabel("Password").fill(env("BUIZZ_PASSWORD"));
    await page.getByRole("button", { name: "Sign in" }).click();
    await page.getByRole("link", { name: "Bots", exact: true }).waitFor();
    console.log(JSON.stringify(await run(page)));
  } finally {
    await browser.close();
  }
}
