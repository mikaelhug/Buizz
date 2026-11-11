import { useEffect, useState } from "react";
import { api, errorMessage, must, mustSucceed, UNAUTHORIZED_EVENT } from "./api/client";
import { connectStream, useStreamStatus } from "./api/stream";
import { href, type Route, useRoute } from "./lib/route";
import { BotDetail } from "./pages/BotDetail";
import { BotGrid } from "./pages/BotGrid";
import { DataHealth } from "./pages/DataHealth";
import { Leaderboard } from "./pages/Leaderboard";
import { Login } from "./pages/Login";
import { DialogHost } from "./ui/Dialog";
import { notify, Toaster } from "./ui/Toaster";

type Auth = "checking" | "in" | "out";

export function App() {
  const [auth, setAuth] = useState<Auth>("checking");

  useEffect(() => {
    must(api.GET("/api/auth/session")).then(
      (session) => setAuth(session.authenticated ? "in" : "out"),
      () => setAuth("out"),
    );
    const signOut = () => setAuth("out");
    window.addEventListener(UNAUTHORIZED_EVENT, signOut);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, signOut);
  }, []);

  return (
    <>
      {auth === "in" && <Shell onLoggedOut={() => setAuth("out")} />}
      {auth === "out" && <Login onLoggedIn={() => setAuth("in")} />}
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

const STREAM_TITLES = {
  open: "Live updates connected",
  connecting: "Connecting live updates",
  down: "Live updates disconnected; retrying",
};

function Shell({ onLoggedOut }: { onLoggedOut: () => void }) {
  const route = useRoute();
  const stream = useStreamStatus();

  useEffect(() => connectStream(), []);

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
          Buizz
        </a>
        <nav className="nav">
          {NAV.map((item) => (
            <a key={item.text} href={href(item.route)} className={item.page.includes(route.page) ? "active" : undefined}>
              {item.text}
            </a>
          ))}
        </nav>
        <span className={`stream-dot stream-${stream}`} title={STREAM_TITLES[stream]} role="img" aria-label={STREAM_TITLES[stream]} />
        <button type="button" className="btn btn-ghost btn-sm" onClick={() => void logout()}>
          Sign out
        </button>
      </header>
      <main className="page">
        {route.page === "bots" && <BotGrid />}
        {route.page === "bot" && <BotDetail key={route.name} name={route.name} runId={route.run} />}
        {route.page === "leaderboard" && <Leaderboard preset={route.preset} />}
        {route.page === "data" && <DataHealth />}
      </main>
    </div>
  );
}
