import { useEffect, useEffectEvent, useSyncExternalStore } from "react";
import { api, type Schemas, UNAUTHORIZED_EVENT } from "./client";

export type SpineEvent = Schemas["SpineEvent"];
export type StreamStatus = "connecting" | "open" | "down";

const EVENT_TYPES: Record<SpineEvent["type"], true> = {
  run_status: true,
  live_session: true,
  equity: true,
  order: true,
  fill: true,
  log: true,
  job: true,
  feed: true,
  contracts: true,
  usage: true,
  unscreened: true,
};

const RETRY_MS = 3000;

const eventListeners = new Set<(event: SpineEvent) => void>();
const resyncListeners = new Set<() => void>();
const statusListeners = new Set<() => void>();
let status: StreamStatus = "connecting";

function setStatus(next: StreamStatus) {
  status = next;
  statusListeners.forEach((listener) => listener());
}

export function connectStream(): () => void {
  let source: EventSource | undefined;
  let retry: ReturnType<typeof setTimeout> | undefined;
  let openedBefore = false;
  let stopped = false;

  const retryUnlessSignedOut = async () => {
    const { data } = await api.GET("/api/auth/session").catch(() => ({ data: undefined }));
    if (stopped) {
      return;
    }
    if (data?.authenticated === false) {
      window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
      return;
    }
    retry = setTimeout(open, RETRY_MS);
  };

  const open = () => {
    setStatus("connecting");
    source = new EventSource("/api/events", { withCredentials: true });
    source.onopen = () => {
      if (openedBefore) {
        resyncListeners.forEach((listener) => listener());
      }
      openedBefore = true;
      setStatus("open");
    };
    source.onerror = () => {
      if (source?.readyState === EventSource.CLOSED) {
        setStatus("down");
        void retryUnlessSignedOut();
      } else {
        setStatus("connecting");
      }
    };
    for (const type of Object.keys(EVENT_TYPES)) {
      source.addEventListener(type, (message) => {
        const event = JSON.parse((message as MessageEvent<string>).data) as SpineEvent;
        eventListeners.forEach((listener) => listener(event));
      });
    }
  };

  open();
  return () => {
    stopped = true;
    clearTimeout(retry);
    source?.close();
    setStatus("connecting");
  };
}

export function useSpineEvents(handler: (event: SpineEvent) => void) {
  const onEvent = useEffectEvent(handler);
  useEffect(() => {
    const listener = (event: SpineEvent) => onEvent(event);
    eventListeners.add(listener);
    return () => {
      eventListeners.delete(listener);
    };
  }, []);
}

export function onResync(listener: () => void): () => void {
  resyncListeners.add(listener);
  return () => {
    resyncListeners.delete(listener);
  };
}

export function useStreamStatus(): StreamStatus {
  return useSyncExternalStore(
    (listener) => {
      statusListeners.add(listener);
      return () => {
        statusListeners.delete(listener);
      };
    },
    () => status,
  );
}
