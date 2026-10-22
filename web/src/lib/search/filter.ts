import { rowProfit, type Row } from "./rows";

/** Result filters; all off shows everything. Not stored (decided 2026-10-22). */
export type Filters = {
  /** Has an Amazon match (accepted or needs review). */
  matched: boolean;
  /** Profit above zero with the user's costs. */
  profitable: boolean;
  /** In stock when seen. */
  inStock: boolean;
};

export type FilterKey = keyof Filters;

export const FILTER_KEYS: readonly FilterKey[] = ["matched", "profitable", "inStock"];

export const NO_FILTERS: Filters = { matched: false, profitable: false, inStock: false };

/** Rows passing every active filter, in their given order. */
export function filterRows(rows: readonly Row[], filters: Filters): Row[] {
  return rows.filter((row) => {
    if (filters.matched && !row.result.amazon) return false;
    if (filters.profitable && !((rowProfit(row) ?? 0) > 0)) return false;
    if (filters.inStock && row.result.availability !== "in_stock") return false;
    return true;
  });
}
