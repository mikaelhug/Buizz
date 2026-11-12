import { useEffect, useSyncExternalStore } from "react";
import type { Schemas } from "../api/client";

export type Preset = Schemas["PeriodPreset"];

export const PRESETS: readonly Preset[] = ["1y", "3y", "5y", "10y", "holdout"];

export const BOT_TABS = ["overview", "trades", "orders", "events", "logs", "runs"] as const;

export type BotTab = (typeof BOT_TABS)[number];

export type Route =
  | { page: "bots" }
  | { page: "bot"; name: string; run?: number; tab?: BotTab }
  | { page: "leaderboard"; preset: Preset }
  | { page: "data" };

function isPreset(value: string | undefined): value is Preset {
  return PRESETS.some((preset) => preset === value);
}

function isBotTab(value: string | undefined): value is BotTab {
  return BOT_TABS.some((tab) => tab === value && tab !== "overview");
}

function botRoute(name: string, rest: string[]): Route {
  const [sub, id, last] = rest;
  const run = sub === "runs" ? Number(id) : Number.NaN;
  const tab = Number.isInteger(run) ? last : sub;
  return {
    page: "bot",
    name,
    ...(Number.isInteger(run) ? { run } : {}),
    ...(isBotTab(tab) ? { tab } : {}),
  };
}

function parse(hash: string): Route {
  const [page, name, ...rest] = hash.replace(/^#\/?/, "").split("/").map(decodeURIComponent);
  if (page === "bots" && name) {
    return botRoute(name, rest);
  }
  if (page === "leaderboard") {
    return { page: "leaderboard", preset: isPreset(name) ? name : "1y" };
  }
  if (page === "data") {
    return { page: "data" };
  }
  return { page: "bots" };
}

export function href(route: Route): string {
  switch (route.page) {
    case "bots":
      return "#/";
    case "bot": {
      const base = `#/bots/${encodeURIComponent(route.name)}`;
      const run = route.run === undefined ? base : `${base}/runs/${route.run}`;
      return route.tab === undefined || route.tab === "overview" ? run : `${run}/${route.tab}`;
    }
    case "leaderboard":
      return `#/leaderboard/${route.preset}`;
    case "data":
      return "#/data";
  }
}

export function navigate(route: Route) {
  window.location.hash = href(route);
}

function subscribe(listener: () => void) {
  window.addEventListener("hashchange", listener);
  return () => window.removeEventListener("hashchange", listener);
}

export function useRoute(): Route {
  const hash = useSyncExternalStore(subscribe, () => window.location.hash);
  return parse(hash);
}

export function routeTitle(route: Route): string {
  switch (route.page) {
    case "bots":
      return "Bots";
    case "bot":
      return route.run === undefined ? route.name : `${route.name} · Run #${route.run}`;
    case "leaderboard":
      return `Leaderboard · ${route.preset}`;
    case "data":
      return "Data";
  }
}

export function useDocumentTitle(title: string) {
  useEffect(() => {
    document.title = `${title} · Buizz`;
  }, [title]);
}
