import { fmt } from "../lib/format";

type Shape = "block" | "chart" | "cards" | "table";

const REPEATS: Partial<Record<Shape, number>> = { cards: 6, table: 6 };

export function Loading({ shape = "block" }: { shape?: Shape }) {
  const repeats = REPEATS[shape];
  return (
    <div className={repeats ? `skeleton-${shape}` : `skeleton skeleton-${shape}`} role="status" aria-busy="true">
      <span className="visually-hidden">Loading</span>
      {Array.from({ length: repeats ?? 0 }, (_, index) => (
        <div key={index} className="skeleton" />
      ))}
    </div>
  );
}

export function Failure({ message, onRetry }: { message: string; onRetry: () => void }) {
  return (
    <div className="failure" role="alert">
      <span>{message}</span>
      <button type="button" className="btn btn-sm" onClick={onRetry}>
        Retry
      </button>
    </div>
  );
}

export function StaleFailure({ resource }: { resource: { data: unknown; error: string | undefined; reload: () => void } }) {
  return resource.data !== undefined && resource.error ? <Failure message={resource.error} onRetry={resource.reload} /> : null;
}

export function LoadMore({ onClick, loaded, noun }: { onClick: (() => void) | undefined; loaded: number; noun: string }) {
  return onClick ? (
    <div className="load-more">
      <button type="button" className="btn btn-sm" onClick={onClick}>
        Load more
      </button>
      <span className="muted">
        Sorting and filtering cover the {fmt.integer(loaded)} {noun} loaded so far
      </span>
    </div>
  ) : null;
}
