import { rowProfit, rowRoi, type Row } from "./rows";

export type SortKey = "price" | "profit" | "roi";
export type SortDir = "asc" | "desc";
export type Sort = { key: SortKey; dir: SortDir };

/** Decided 2026-10-22: most profitable first. */
export const DEFAULT_SORT: Sort = { key: "profit", dir: "desc" };

/** Columns that sort, in display order. */
export const SORT_KEYS: readonly SortKey[] = ["price", "profit", "roi"];

function value(row: Row, key: SortKey): number | null {
  switch (key) {
    case "price":
      return row.result.price_centavos;
    case "profit":
      return rowProfit(row);
    case "roi":
      return rowRoi(row);
  }
}

/**
 * Sorts a copy of `rows`. Rows without a value go last in both
 * directions; ties keep the API order (retailer, then SKU).
 */
export function sortRows(rows: readonly Row[], sort: Sort): Row[] {
  const sign = sort.dir === "asc" ? 1 : -1;
  return [...rows].sort((a, b) => {
    const va = value(a, sort.key);
    const vb = value(b, sort.key);
    if (va === null || vb === null) {
      if (va !== vb) return va === null ? 1 : -1;
      return a.index - b.index;
    }
    return va === vb ? a.index - b.index : (va - vb) * sign;
  });
}

/**
 * The sort after activating `key`'s header: the same column flips
 * direction; a new column starts with its natural direction (cheapest
 * price first, highest profit or ROI first).
 */
export function nextSort(current: Sort, key: SortKey): Sort {
  if (current.key === key) return { key, dir: current.dir === "asc" ? "desc" : "asc" };
  return { key, dir: key === "price" ? "asc" : "desc" };
}

/** Value of `aria-sort` for `key`'s column header. */
export function ariaSort(sort: Sort, key: SortKey): "ascending" | "descending" | "none" {
  if (sort.key !== key) return "none";
  return sort.dir === "asc" ? "ascending" : "descending";
}
