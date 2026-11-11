export function Loading() {
  return <div className="loading" aria-busy="true" />;
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

export function LoadMore({ onClick }: { onClick: (() => void) | undefined }) {
  return onClick ? (
    <button type="button" className="btn btn-sm load-more" onClick={onClick}>
      Load more
    </button>
  ) : null;
}
