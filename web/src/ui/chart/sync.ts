import type { Preset } from "./model";

export type Span = { from: number; to: number; preset: Preset | null };

type Peer = { crosshair: (time: number | undefined) => void; span: (span: Span) => void };

export type Link = Peer & { leave: () => void };

export type ChartSync = { join: (peer: Peer) => Link };

export function chartSync(): ChartSync {
  const peers = new Set<Peer>();
  return {
    join(peer) {
      peers.add(peer);
      const others = () => [...peers].filter((other) => other !== peer);
      return {
        crosshair: (time) => others().forEach((other) => other.crosshair(time)),
        span: (span) => others().forEach((other) => other.span(span)),
        leave: () => {
          peers.delete(peer);
        },
      };
    },
  };
}
