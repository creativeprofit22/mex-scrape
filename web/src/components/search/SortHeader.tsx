import { ariaSort, type Sort, type SortKey } from "@/lib/search/sort";

/**
 * A sortable column header. `aria-sort` belongs on the header cell, so
 * this renders the `th` itself with a button inside (see sources#sort-header).
 * The direction arrow is drawn in CSS; `aria-sort` carries the state.
 */
export function SortHeader({
  column,
  label,
  sort,
  onSort,
  className,
}: {
  column: SortKey;
  label: string;
  sort: Sort;
  onSort: (column: SortKey) => void;
  className?: string;
}) {
  const state = ariaSort(sort, column);
  return (
    <th scope="col" aria-sort={state} className={className}>
      <button
        type="button"
        className="plain-button sort-button"
        data-sort={state}
        onClick={() => onSort(column)}
      >
        {label}
      </button>
    </th>
  );
}
