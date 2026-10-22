import type { Dictionary } from "@/i18n/dictionaries";
import { interpolate } from "@/i18n/format";
import { FILTER_KEYS, type FilterKey, type Filters } from "@/lib/search/filter";

/**
 * Toggle chips (`aria-pressed`; the check glyph is drawn in CSS, see
 * sources#kinetics-chips) and the shown count, announced via `role=status`.
 */
export function FilterChips({
  dict,
  filters,
  onToggle,
  shown,
  total,
}: {
  dict: Dictionary["search"]["filters"];
  filters: Filters;
  onToggle: (key: FilterKey) => void;
  shown: number;
  total: number;
}) {
  return (
    <div className="filter-bar">
      <div className="chips" role="group" aria-label={dict.label}>
        {FILTER_KEYS.map((key) => (
          <button
            key={key}
            type="button"
            className="plain-button chip"
            aria-pressed={filters[key]}
            onClick={() => onToggle(key)}
          >
            {dict[key]}
          </button>
        ))}
      </div>
      <p className="result-count" role="status" data-testid="result-count">
        {interpolate(dict.count, { shown, total })}
      </p>
    </div>
  );
}
