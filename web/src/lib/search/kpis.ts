import type { Schemas } from "@/lib/api/client";

import { rowProfit, type Row } from "./rows";

export type Kpis = {
  /** Highest profit across the whole search, or `null` when none has one. */
  bestProfit: { centavos: number; key: string } | null;
  /** Offers shown (after membership hiding). */
  offers: number;
  /** Stores that returned offers, out of those asked. */
  stores: { withOffers: number; total: number };
  /** Offers hidden because they need a membership the user lacks. */
  hidden: number;
};

/**
 * Summary tiles for a search. Always the whole search, whatever filters
 * are on (decided 2026-10-22). Ties on profit keep the earliest row.
 */
export function deriveKpis(
  search: Pick<Schemas["SearchResponse"], "results" | "retailers" | "hidden_results">,
  rows: readonly Row[],
): Kpis {
  let bestProfit: Kpis["bestProfit"] = null;
  for (const row of rows) {
    const profit = rowProfit(row);
    if (profit === null) continue;
    if (bestProfit === null || profit > bestProfit.centavos) {
      bestProfit = { centavos: profit, key: row.key };
    }
  }
  return {
    bestProfit,
    offers: search.results.length,
    stores: {
      withOffers: search.retailers.filter((r) => r.offers > 0).length,
      total: search.retailers.length,
    },
    hidden: search.hidden_results,
  };
}
