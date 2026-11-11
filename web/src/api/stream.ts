import { useEffect, useEffectEvent, useSyncExternalStore } from "react";
import type { Schemas } from "./client";

export type SpineEvent = Schemas["SpineEvent"];
export type StreamStatus = "connecting" | "open" | "down";

const EVENT_TYPES: Record<SpineEvent["type"], true> = {
  run_status: true,
  equity: true,
  order: true,
  fill: true,
  log: true,
  job: true,
  feed: true,
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
        retry = setTimeout(open, RETRY_MS);
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
