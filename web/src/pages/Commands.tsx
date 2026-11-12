import { type KeyboardEvent as ReactKeyboardEvent, useEffect, useEffectEvent, useId, useMemo, useRef, useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { fmt, label } from "../lib/format";
import { navigate, type Route } from "../lib/route";
import { setThemePreference } from "../lib/theme";
import { goLive, isRunning, pauseRun, resumeRun, undoPause } from "./BotActions";
import { launchBacktest, launchSweep } from "./Launch";
import { Icon } from "../ui/Icon";
import { notify } from "../ui/Toaster";

type Command = { id: string; group: string; title: string; hint?: string | undefined; keys?: string[]; run: () => void };
type Catalog = { bots: Schemas["BotSummary"][]; runs: Schemas["RunSummary"][] };

const RECENT_RUNS = 30;
const RECENT_SHOWN = 6;
const CHORD_MS = 1200;

export const SHORTCUTS: { keys: string[]; text: string }[] = [
  { keys: ["⌘", "K"], text: "Open the command palette" },
  { keys: ["/"], text: "Search the page, or open the palette" },
  { keys: ["?"], text: "Show keyboard shortcuts" },
  { keys: ["G", "B"], text: "Go to Bots" },
  { keys: ["G", "L"], text: "Go to League" },
  { keys: ["G", "D"], text: "Go to Data" },
  { keys: ["G", "R"], text: "Research of the bot you are on" },
  { keys: ["B"], text: "Backtest the bot you are on" },
  { keys: ["S"], text: "Sweep the bot you are on" },
  { keys: ["←", "→"], text: "Switch run tabs" },
  { keys: ["Shift", "Click"], text: "Select a range of table rows" },
  { keys: ["Esc"], text: "Close, or clear the selection" },
];

function botOf(route: Route): string | undefined {
  return route.page === "bot" || route.page === "research" ? route.name : undefined;
}

function report(pending: Promise<Schemas["RunSummary"]>, done: string, undo = false) {
  pending.then(
    (run) => notify(`${run.bot} ${done} · run #${run.id}`, "success", undo ? undoPause([run]) : undefined),
    (error: unknown) => notify(errorMessage(error)),
  );
}

function liveCommands(bot: Schemas["BotSummary"]): Command[] {
  return bot.live.map(({ run }) =>
    isRunning(run)
      ? { id: `pause-${run.id}`, group: "Live runs", title: `Pause ${bot.name} #${run.id}`, run: () => report(pauseRun(run.id), "paused", true) }
      : { id: `resume-${run.id}`, group: "Live runs", title: `Resume ${bot.name} #${run.id}`, run: () => report(resumeRun(run.id), "live") },
  );
}

function botActions(bot: Schemas["BotSummary"], group: string): Command[] {
  return [
    { id: `backtest-${bot.name}`, group, title: `Backtest ${bot.name}…`, keys: group === "This bot" ? ["B"] : [], run: () => launchBacktest(bot.name) },
    { id: `sweep-${bot.name}`, group, title: `Sweep ${bot.name}…`, keys: group === "This bot" ? ["S"] : [], run: () => launchSweep(bot.name) },
    { id: `research-${bot.name}`, group, title: `Research ${bot.name}`, keys: group === "This bot" ? ["G", "R"] : [], run: () => navigate({ page: "research", name: bot.name }) },
    ...(bot.live.length === 0 ? [{ id: `live-${bot.name}`, group, title: `Go live with ${bot.name}`, hint: "default params", run: () => report(goLive(bot.name), "live") }] : []),
    ...liveCommands(bot),
  ];
}

function commands(catalog: Catalog | undefined, route: Route, query: boolean, actions: { help: () => void; signOut: () => void }): Command[] {
  const current = botOf(route);
  const bots = catalog?.bots ?? [];
  const here = bots.find((bot) => bot.name === current);
  const runs = catalog?.runs ?? [];
  return [
    ...(here ? botActions(here, "This bot") : []),
    { id: "go-bots", group: "Go to", title: "Bots", keys: ["G", "B"], run: () => navigate({ page: "bots" }) },
    { id: "go-league", group: "Go to", title: "League", keys: ["G", "L"], run: () => navigate({ page: "league" }) },
    { id: "go-data", group: "Go to", title: "Data", keys: ["G", "D"], run: () => navigate({ page: "data" }) },
    ...bots.map((bot) => ({
      id: `bot-${bot.name}`,
      group: "Bots",
      title: bot.name,
      hint: [bot.timeframe, bot.live.length > 0 ? `${bot.live.length} live` : label(bot.stage)].join(" · "),
      run: () => navigate({ page: "bot", name: bot.name }),
    })),
    ...(query ? bots.filter((bot) => bot !== here).flatMap((bot) => botActions(bot, "Actions")) : []),
    ...(query ? runs : runs.slice(0, RECENT_SHOWN)).map((run) => ({
      id: `run-${run.id}`,
      group: "Recent runs",
      title: `${run.bot} #${run.id}`,
      hint: [run.mode, label(run.status), ...(run.metrics ? [fmt.change(run.metrics.total_return)] : [])].join(" · "),
      run: () => navigate({ page: "bot", name: run.bot, run: run.id }),
    })),
    { id: "theme-light", group: "Preferences", title: "Theme: light", run: () => setThemePreference("light") },
    { id: "theme-dark", group: "Preferences", title: "Theme: dark", run: () => setThemePreference("dark") },
    { id: "theme-system", group: "Preferences", title: "Theme: system", run: () => setThemePreference("system") },
    { id: "help", group: "Preferences", title: "Keyboard shortcuts", keys: ["?"], run: actions.help },
    { id: "sign-out", group: "Preferences", title: "Sign out", run: actions.signOut },
  ];
}

function score(command: Command, needle: string): number {
  const haystack = `${command.title} ${command.hint ?? ""}`.toLowerCase();
  const at = haystack.indexOf(needle);
  if (at >= 0) {
    return 1000 - at * 2 - (at === 0 || haystack[at - 1] === " " ? 0 : 50);
  }
  let from = 0;
  for (const letter of needle) {
    from = haystack.indexOf(letter, from) + 1;
    if (from === 0) {
      return -1;
    }
  }
  return 100 - from;
}

function filtered(all: Command[], query: string): Command[] {
  const needle = query.trim().toLowerCase();
  if (!needle) {
    return all;
  }
  return all
    .map((command) => ({ command, score: score(command, needle) }))
    .filter((entry) => entry.score >= 0)
    .toSorted((a, b) => b.score - a.score)
    .map((entry) => entry.command);
}

export function Keys({ keys }: { keys: string[] }) {
  return (
    <span className="keys">
      {keys.map((key) => (
        <kbd key={key}>{key}</kbd>
      ))}
    </span>
  );
}

function Palette({ route, onClose, actions }: { route: Route; onClose: () => void; actions: { help: () => void; signOut: () => void } }) {
  const dialog = useRef<HTMLDialogElement>(null);
  const list = useRef<HTMLUListElement>(null);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const [catalog, setCatalog] = useState<Catalog>();
  const id = useId();

  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    let live = true;
    Promise.all([must(api.GET("/api/bots")), must(api.GET("/api/runs", { params: { query: { limit: RECENT_RUNS } } }))]).then(
      ([bots, runs]) => live && setCatalog({ bots, runs }),
      (error: unknown) => live && notify(errorMessage(error)),
    );
    return () => {
      live = false;
      element?.close();
    };
  }, []);

  const shown = useMemo(() => filtered(commands(catalog, route, query.trim() !== "", actions), query), [catalog, route, query, actions]);
  const current = Math.min(active, shown.length - 1);

  useEffect(() => {
    list.current?.querySelector<HTMLElement>(`[data-index="${current}"]`)?.scrollIntoView({ block: "nearest" });
  }, [current]);

  const choose = (command: Command | undefined) => {
    if (command) {
      onClose();
      command.run();
    }
  };

  const onKeyDown = (event: ReactKeyboardEvent) => {
    const moves: Record<string, number> = { ArrowDown: current + 1, ArrowUp: current - 1, PageDown: current + 8, PageUp: current - 8 };
    const target = moves[event.key];
    if (target !== undefined && shown.length > 0) {
      event.preventDefault();
      setActive((target + shown.length) % shown.length);
    } else if (event.key === "Enter") {
      event.preventDefault();
      choose(shown[current]);
    }
  };

  return (
    <dialog
      ref={dialog}
      className="dialog palette"
      aria-label="Command palette"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onClick={(event) => event.target === event.currentTarget && onClose()}
    >
      <div className="palette-body">
        <input
          className="palette-input"
          type="text"
          role="combobox"
          aria-expanded="true"
          aria-controls={`${id}-list`}
          aria-activedescendant={shown[current] ? `${id}-${shown[current].id}` : undefined}
          aria-label="Search pages, bots, runs and actions"
          placeholder="Jump to a bot or run, or type an action…"
          autoFocus
          value={query}
          onChange={(event) => {
            setQuery(event.target.value);
            setActive(0);
          }}
          onKeyDown={onKeyDown}
        />
        <ul ref={list} id={`${id}-list`} className="palette-list" role="listbox" aria-label="Commands">
          {shown.length === 0 && <li className="palette-empty">{catalog ? `Nothing matches “${query.trim()}”` : "Loading bots and runs…"}</li>}
          {shown.map((command, index) => (
            <li key={command.id} role="presentation">
              {(index === 0 || shown[index - 1]?.group !== command.group) && !query.trim() && <div className="palette-group">{command.group}</div>}
              <div
                id={`${id}-${command.id}`}
                role="option"
                aria-selected={index === current}
                data-index={index}
                className="palette-item"
                onPointerMove={() => index !== current && setActive(index)}
                onClick={() => choose(command)}
              >
                <span className="palette-title">{command.title}</span>
                {command.hint && <span className="palette-hint">{command.hint}</span>}
                {query.trim() && <span className="palette-hint">{command.group}</span>}
                {command.keys && command.keys.length > 0 && <Keys keys={command.keys} />}
              </div>
            </li>
          ))}
        </ul>
        <footer className="palette-foot">
          <Keys keys={["↑", "↓"]} /> move <Keys keys={["↵"]} /> open <Keys keys={["Esc"]} /> close
        </footer>
      </div>
    </dialog>
  );
}

function Help({ onClose }: { onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const element = dialog.current;
    element?.showModal();
    return () => element?.close();
  }, []);
  return (
    <dialog
      ref={dialog}
      className="dialog"
      aria-label="Keyboard shortcuts"
      onCancel={(event) => {
        event.preventDefault();
        onClose();
      }}
      onClick={(event) => event.target === event.currentTarget && onClose()}
    >
      <div className="dialog-body">
        <h2 className="dialog-title">Keyboard shortcuts</h2>
        <dl className="shortcuts">
          {SHORTCUTS.map((shortcut) => (
            <div key={shortcut.text}>
              <dt>
                <Keys keys={shortcut.keys} />
              </dt>
              <dd>{shortcut.text}</dd>
            </div>
          ))}
        </dl>
        <div className="dialog-actions">
          <button type="button" className="btn" onClick={onClose} autoFocus>
            Close
          </button>
        </div>
      </div>
    </dialog>
  );
}

function typing(target: EventTarget | null): boolean {
  if (target instanceof HTMLInputElement) {
    return !["checkbox", "radio", "button", "submit", "reset"].includes(target.type);
  }
  return target instanceof HTMLElement && (target.isContentEditable || target.tagName === "TEXTAREA" || target.tagName === "SELECT");
}

export function CommandCenter({ route, onSignOut }: { route: Route; onSignOut: () => void }) {
  const [open, setOpen] = useState<"palette" | "help">();
  const actions = useMemo(() => ({ help: () => setOpen("help"), signOut: onSignOut }), [onSignOut]);

  const chordUntil = useRef(0);

  const onKey = useEffectEvent((event: KeyboardEvent) => {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "k") {
      event.preventDefault();
      setOpen((current) => (current === "palette" ? undefined : "palette"));
      return;
    }
    if (open || event.metaKey || event.ctrlKey || event.altKey || typing(event.target) || document.querySelector("dialog[open]")) {
      return;
    }
    const bot = botOf(route);
    const chord = event.timeStamp < chordUntil.current;
    chordUntil.current = 0;
    if (chord) {
      const target: Route | undefined = { b: { page: "bots" as const }, l: { page: "league" as const }, d: { page: "data" as const }, r: bot ? { page: "research" as const, name: bot } : undefined }[
        event.key.toLowerCase()
      ];
      if (target) {
        event.preventDefault();
        navigate(target);
      }
      return;
    }
    if (event.key === "?") {
      event.preventDefault();
      setOpen("help");
    } else if (event.key === "/") {
      event.preventDefault();
      const search = document.querySelector<HTMLInputElement>("[data-page-search]");
      if (search) {
        search.focus();
        search.select();
      } else {
        setOpen("palette");
      }
    } else if (event.key === "g") {
      chordUntil.current = event.timeStamp + CHORD_MS;
    } else if (event.key === "b" && bot) {
      launchBacktest(bot);
    } else if (event.key === "s" && bot) {
      launchSweep(bot);
    }
  });

  useEffect(() => {
    const listener = (event: KeyboardEvent) => onKey(event);
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, []);

  return (
    <>
      <button type="button" className="command-trigger" aria-label="Search or run a command" aria-keyshortcuts="Meta+K Control+K" onClick={() => setOpen("palette")}>
        <Icon name="search" />
        <span className="command-trigger-text">Search or run a command</span>
        <Keys keys={["⌘", "K"]} />
      </button>
      {open === "palette" && <Palette route={route} onClose={() => setOpen(undefined)} actions={actions} />}
      {open === "help" && <Help onClose={() => setOpen(undefined)} />}
    </>
  );
}
