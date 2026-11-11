import { type DependencyList, useCallback, useEffect, useEffectEvent, useRef, useState } from "react";
import { errorMessage } from "../api/client";
import { onResync } from "../api/stream";

export type Resource<T> = {
  data: T | undefined;
  error: string | undefined;
  reload: () => void;
  mutate: (update: (current: T) => T) => void;
};

type State<T> = { data?: T; error?: string };

export function useResource<T>(load: () => Promise<T>, deps: DependencyList): Resource<T> {
  const [state, setState] = useState<State<T>>({});
  const fetchLatest = useRef(() => {});
  const loadLatest = useEffectEvent(load);

  useEffect(() => {
    setState((current) => (current.data === undefined && current.error === undefined ? current : {}));
    let live = true;
    let inFlight = false;
    let again = false;
    const fetchOnce = () => {
      if (inFlight) {
        again = true;
        return;
      }
      inFlight = true;
      loadLatest()
        .then(
          (data) => live && setState({ data }),
          (error: unknown) => live && setState((current) => ({ ...current, error: errorMessage(error) })),
        )
        .finally(() => {
          inFlight = false;
          if (live && again) {
            again = false;
            fetchOnce();
          }
        });
    };
    fetchLatest.current = fetchOnce;
    fetchOnce();
    const stopResync = onResync(fetchOnce);
    return () => {
      live = false;
      stopResync();
    };
  }, deps);

  const reload = useCallback(() => fetchLatest.current(), []);
  const mutate = useCallback(
    (update: (current: T) => T) =>
      setState((current) => {
        if (current.data === undefined) {
          return current;
        }
        const data = update(current.data);
        return data === current.data ? current : { ...current, data };
      }),
    [],
  );

  return { data: state.data, error: state.error, reload, mutate };
}
