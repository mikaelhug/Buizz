import { useEffect, useState } from "react";
import { api, errorMessage, must, type Schemas } from "../api/client";
import { navigate } from "../lib/route";
import { notify } from "../ui/Toaster";
import { BacktestDialog } from "./BacktestDialog";
import { isPreset, type Preset, SweepDialog } from "./SweepDialog";

type Kind = "backtest" | "sweep";
type Request = { kind: Kind; name: string; preset: Preset | undefined };
type Opened = Request & { detail: Schemas["BotDetail"] };

const LAUNCH_EVENT = "buizz-launch";

function launch(request: Request) {
  window.dispatchEvent(new CustomEvent(LAUNCH_EVENT, { detail: request }));
}

export function launchBacktest(name: string) {
  launch({ kind: "backtest", name, preset: undefined });
}

export function launchSweep(name: string, preset?: Preset) {
  launch({ kind: "sweep", name, preset });
}

function screenedPreset(detail: Schemas["BotDetail"]): Preset {
  const preset = detail.summary.screening?.preset;
  return isPreset(preset) ? preset : "10y";
}

export function LaunchHost() {
  const [opened, setOpened] = useState<Opened>();

  useEffect(() => {
    let latest = 0;
    const onLaunch = (event: Event) => {
      const request = (event as CustomEvent<Request>).detail;
      const ticket = ++latest;
      must(api.GET("/api/bots/{name}", { params: { path: { name: request.name } } })).then(
        (detail) => ticket === latest && setOpened({ ...request, detail }),
        (error: unknown) => notify(`${request.name}: ${errorMessage(error)}`),
      );
    };
    window.addEventListener(LAUNCH_EVENT, onLaunch);
    return () => window.removeEventListener(LAUNCH_EVENT, onLaunch);
  }, []);

  if (!opened) {
    return null;
  }
  const { kind, name, detail } = opened;
  const close = () => setOpened(undefined);
  return kind === "backtest" ? (
    <BacktestDialog
      name={name}
      holdoutRuns={detail.holdout_runs}
      risk={detail.manifest.risk ?? {}}
      onClose={close}
      onStarted={(run) => {
        close();
        navigate({ page: "bot", name, run: run.id });
      }}
    />
  ) : (
    <SweepDialog
      name={name}
      defaults={detail.summary.params as Record<string, unknown>}
      preset={opened.preset ?? screenedPreset(detail)}
      onClose={close}
      onStarted={(_, preset) => {
        close();
        navigate({ page: "research", name, preset });
      }}
    />
  );
}
