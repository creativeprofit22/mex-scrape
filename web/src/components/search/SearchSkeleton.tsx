const ROWS = 5;

/**
 * Static placeholder rows while a search runs. Fills come from existing
 * tokens so they follow light and dark; no shimmer (style-pack §2.3 has
 * no ambient motion; see sources#antd-skeleton). Hidden from assistive
 * tech: the status line announces progress.
 */
export function SearchSkeleton() {
  return (
    <div className="results results--skeleton" aria-hidden="true">
      {Array.from({ length: ROWS }, (_, i) => (
        <div key={i} className="skeleton-row">
          <span className="skeleton skeleton--title" />
          <span className="skeleton skeleton--value" />
          <span className="skeleton skeleton--value" />
        </div>
      ))}
    </div>
  );
}
