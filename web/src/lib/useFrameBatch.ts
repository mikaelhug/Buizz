import { useCallback, useEffect, useEffectEvent, useRef } from "react";

export function useFrameBatch<T>(flush: (items: T[]) => void): (item: T) => void {
  const onFlush = useEffectEvent(flush);
  const pending = useRef<T[]>([]);
  const frame = useRef<number | undefined>(undefined);

  useEffect(
    () => () => {
      if (frame.current !== undefined) {
        cancelAnimationFrame(frame.current);
        frame.current = undefined;
      }
      pending.current = [];
    },
    [],
  );

  return useCallback((item: T) => {
    pending.current.push(item);
    frame.current ??= requestAnimationFrame(() => {
      frame.current = undefined;
      const items = pending.current;
      pending.current = [];
      onFlush(items);
    });
  }, []);
}
