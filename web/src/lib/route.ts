import { useEffect, useSyncExternalStore } from "react";

export const BOT_TABS = ["overview", "trades", "orders", "events", "logs", "runs"] as const;

export type BotTab = (typeof BOT_TABS)[number];

export type Route =
  | { page: "bots" }
  | { page: "bot"; name: string; run?: number; tab?: BotTab; symbol?: string }
  | { page: "research"; name: string; preset?: string }
  | { page: "league" }
  | { page: "data" };

function isBotTab(value: string | undefined): value is BotTab {
  return BOT_TABS.some((tab) => tab === value && tab !== "overview");
}

function botRoute(name: string, rest: string[]): Route {
  const [sub, id] = rest;
  if (sub === "research") {
    return { page: "research", name, ...(id ? { preset: id } : {}) };
  }
  const run = sub === "runs" ? Number(id) : Number.NaN;
  const [view, symbol] = Number.isInteger(run) ? rest.slice(2) : rest;
  return {
    page: "bot",
    name,
    ...(Number.isInteger(run) ? { run } : {}),
    ...(view === "symbol" && symbol ? { symbol } : isBotTab(view) ? { tab: view } : {}),
  };
}

function parse(hash: string): Route {
  const [page, name, ...rest] = hash.replace(/^#\/?/, "").split("/").map(decodeURIComponent);
  if (page === "bots" && name) {
    return botRoute(name, rest);
  }
  if (page === "league") {
    return { page: "league" };
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
      if (route.symbol !== undefined) {
        return `${run}/symbol/${encodeURIComponent(route.symbol)}`;
      }
      return route.tab === undefined || route.tab === "overview" ? run : `${run}/${route.tab}`;
    }
    case "research": {
      const base = `#/bots/${encodeURIComponent(route.name)}/research`;
      return route.preset === undefined ? base : `${base}/${encodeURIComponent(route.preset)}`;
    }
    case "league":
      return "#/league";
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
      return [route.name, ...(route.run === undefined ? [] : [`Run #${route.run}`]), ...(route.symbol === undefined ? [] : [route.symbol])].join(" · ");
    case "research":
      return [route.name, "Research", ...(route.preset === undefined ? [] : [route.preset])].join(" · ");
    case "league":
      return "League";
    case "data":
      return "Data";
  }
}

export function useDocumentTitle(title: string) {
  useEffect(() => {
    document.title = `${title} · Buizz`;
  }, [title]);
}
