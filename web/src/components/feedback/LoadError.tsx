/** A page load that failed: says why and offers to try again. */
export function LoadError({
  message,
  retryLabel,
  onRetry,
}: {
  message: string;
  retryLabel: string;
  onRetry: () => void;
}) {
  return (
    <div className="stack load-error">
      <p role="alert" className="error">
        {message}
      </p>
      <button type="button" onClick={onRetry}>
        {retryLabel}
      </button>
    </div>
  );
}
