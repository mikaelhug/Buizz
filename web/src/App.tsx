import { useEffect, useState } from "react";
import { api, ApiFailure, errorMessage, must, mustSucceed, UNAUTHORIZED_EVENT } from "./api/client";
import { connectStream, useStreamStatus } from "./api/stream";
import { href, type Route, routeTitle, useDocumentTitle, useRoute } from "./lib/route";
import { BotDetail } from "./pages/BotDetail";
import { BotGrid } from "./pages/BotGrid";
import { DataHealth } from "./pages/DataHealth";
import { Leaderboard } from "./pages/Leaderboard";
import { Login, Unreachable } from "./pages/Login";
import { DialogHost } from "./ui/Dialog";
import { ErrorBoundary } from "./ui/ErrorBoundary";
import { BrandMark } from "./ui/Icon";
import { ThemeMenu } from "./ui/ThemeMenu";
import { notify, Toaster } from "./ui/Toaster";
import "./styles/shell.css";

type Auth = { state: "checking" | "in" | "out" } | { state: "unreachable"; reason: string; retry: () => void };

const RETRY_MS = { first: 1000, max: 30_000 };

export function App() {
  const [auth, setAuth] = useState<Auth>({ state: "checking" });

  useEffect(() => {
    let live = true;
    let attempt = 0;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const check = () => {
      clearTimeout(timer);
      must(api.GET("/api/auth/session")).then(
        (session) => live && setAuth({ state: session.authenticated ? "in" : "out" }),
        (error: unknown) => {
          if (!live) {
            return;
          }
          if (error instanceof ApiFailure && error.status === 401) {
            setAuth({ state: "out" });
            return;
          }
          setAuth({ state: "unreachable", reason: errorMessage(error), retry: check });
          timer = setTimeout(check, Math.min(RETRY_MS.first * 2 ** attempt, RETRY_MS.max));
          attempt += 1;
        },
      );
    };
    check();
    const signOut = () => setAuth({ state: "out" });
    window.addEventListener(UNAUTHORIZED_EVENT, signOut);
    return () => {
      live = false;
      clearTimeout(timer);
      window.removeEventListener(UNAUTHORIZED_EVENT, signOut);
    };
  }, []);

  return (
    <>
      {auth.state === "in" && <Shell onLoggedOut={() => setAuth({ state: "out" })} />}
      {auth.state === "out" && <Login onLoggedIn={() => setAuth({ state: "in" })} />}
      {auth.state === "unreachable" && <Unreachable reason={auth.reason} onRetry={auth.retry} />}
      <DialogHost />
      <Toaster />
    </>
  );
}

const NAV: { route: Route; text: string; page: Route["page"][] }[] = [
  { route: { page: "bots" }, text: "Bots", page: ["bots", "bot"] },
  { route: { page: "leaderboard", preset: "1y" }, text: "Leaderboard", page: ["leaderboard"] },
  { route: { page: "data" }, text: "Data", page: ["data"] },
];

const STREAM = {
  open: { text: "Connected", title: "Live updates connected" },
  connecting: { text: "Connecting", title: "Connecting live updates" },
  down: { text: "Offline", title: "Live updates disconnected; retrying" },
};

function Shell({ onLoggedOut }: { onLoggedOut: () => void }) {
  const route = useRoute();
  const stream = useStreamStatus();

  useEffect(() => connectStream(), []);
  useDocumentTitle(routeTitle(route));

  const logout = async () => {
    try {
      await mustSucceed(api.POST("/api/auth/logout"));
      onLoggedOut();
    } catch (error) {
      notify(errorMessage(error));
    }
  };

  return (
    <div className="shell">
      <header className="topbar">
        <a className="brand" href={href({ page: "bots" })}>
          <BrandMark />
          <span className="brand-name">Buizz</span>
        </a>
        <nav className="nav">
          {NAV.map((item) => (
            <a key={item.text} href={href(item.route)} className={item.page.includes(route.page) ? "active" : undefined}>
              {item.text}
            </a>
          ))}
        </nav>
        <div className="topbar-tools">
          <span className="stream" role="status" title={STREAM[stream].title}>
            <span className={`stream-dot stream-${stream}`} aria-hidden="true" />
            <span className="stream-text">{STREAM[stream].text}</span>
          </span>
          <ThemeMenu />
          <button type="button" className="btn btn-ghost btn-sm" onClick={() => void logout()}>
            Sign out
          </button>
        </div>
      </header>
      <main className="page">
        <ErrorBoundary resetOn={href(route)}>
          {route.page === "bots" && <BotGrid />}
          {route.page === "bot" && <BotDetail key={route.name} name={route.name} runId={route.run} />}
          {route.page === "leaderboard" && <Leaderboard preset={route.preset} />}
          {route.page === "data" && <DataHealth />}
        </ErrorBoundary>
      </main>
    </div>
  );
}
