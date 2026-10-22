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

/** A match verdict saved from the detail panel (`POST /matches/decisions`). */
export type MatchDecision = "accept" | "reject";

/**
 * Applies this session's saved verdicts (keyed by `resultKey`) without
 * re-running the search: accept marks the match `accepted`; reject drops the
 * Amazon match and its ROI, so the row reads "Sin coincidencia" and leaves
 * the profit KPIs, as it will on the next search (ADR 0013). Rows without a
 * verdict, or without a match to decide, are returned unchanged.
 */
export function applyDecisions(
  rows: readonly Row[],
  decisions: ReadonlyMap<string, MatchDecision>,
): Row[] {
  if (decisions.size === 0) return [...rows];
  return rows.map((row) => {
    const decision = decisions.get(row.key);
    const amazon = row.result.amazon;
    if (!decision || !amazon) return row;
    if (decision === "reject") {
      return { ...row, result: { ...row.result, amazon: null }, roi: undefined };
    }
    if (amazon.match_status === "accepted") return row;
    return { ...row, result: { ...row.result, amazon: { ...amazon, match_status: "accepted" } } };
  });
}
