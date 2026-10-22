import type { Schemas } from "@/lib/api/client";

export type SearchResult = Schemas["SearchResultResponse"];
export type RoiItem = Schemas["RoiItemResponse"];

/** One result with its ROI (from `POST /roi` with the user's costs). */
export type Row = {
  /** Stable identity: `retailer/retailer_sku` (unique within a search). */
  key: string;
  /** Position in the API's order (retailer, then SKU); the sort tie-break. */
  index: number;
  result: SearchResult;
  roi?: RoiItem;
};

/** Identity of a result, stable across re-sorts, filters and polls. */
export function resultKey(result: Pick<SearchResult, "retailer" | "retailer_sku">): string {
  return `${result.retailer}/${result.retailer_sku}`;
}

/** Pairs each result with its ROI item (keyed by the result's API index). */
export function buildRows(
  results: readonly SearchResult[],
  roi: ReadonlyMap<number, RoiItem>,
): Row[] {
  return results.map((result, index) => ({
    key: resultKey(result),
    index,
    result,
    roi: roi.get(index),
  }));
}

/** Profit per unit in centavos, or `null` when there is none (no match, ROI error). */
export function rowProfit(row: Row): number | null {
  if (row.roi?.error) return null;
  return row.roi?.profit_centavos ?? null;
}

/** ROI in basis points, or `null` when there is none. */
export function rowRoi(row: Row): number | null {
  if (row.roi?.error) return null;
  return row.roi?.roi_bp ?? null;
}
