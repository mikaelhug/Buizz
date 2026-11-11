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
