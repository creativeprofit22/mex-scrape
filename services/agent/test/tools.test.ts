import { describe, expect, it } from "vitest";

import type { AgentPriceHistoryResponse } from "../src/api-client.ts";
import { fitPriceHistory, resolveMatchParams, searchOffersParams } from "../src/tools.ts";

describe("tool schemas match the api's limits", () => {
  it("search_offers query is capped at 100 characters (domain::SearchQuery)", () => {
    expect(searchOffersParams.safeParse({ query: "a".repeat(100) }).success).toBe(true);
    expect(searchOffersParams.safeParse({ query: "a".repeat(101) }).success).toBe(false);
  });

  it("resolve_match retailer_sku is capped at 100 characters (domain::RetailerSku)", () => {
    const base = { retailer: "walmart_mx", asin: "B0E2ETEST1", decision: "accept", reason: "same product" };
    expect(resolveMatchParams.safeParse({ ...base, retailer_sku: "S".repeat(100) }).success).toBe(true);
    expect(resolveMatchParams.safeParse({ ...base, retailer_sku: "S".repeat(101) }).success).toBe(false);
    expect(resolveMatchParams.safeParse({ ...base, retailer: "Walmart-MX", retailer_sku: "S1" }).success).toBe(false);
  });
});

function history(days: number, listings: number): AgentPriceHistoryResponse {
  const stats = {
    asin: "B0E2ETEST1",
    as_of_ms: 0,
    hidden_listings: 0,
    listings: Array.from({ length: listings }, (_, i) => ({
      retailer: `r${i}`,
      retailer_sku: `S${i}`,
      windows: [{ days: 365, low_centavos: 94_900 + i, high_centavos: 120_000, average_centavos: 100_000, observations: days }],
    })),
  };
  const series = {
    asin: "B0E2ETEST1",
    as_of_ms: 0,
    since_ms: 0,
    hidden_listings: 0,
    listings: stats.listings.map((l) => ({
      retailer: l.retailer,
      retailer_sku: l.retailer_sku,
      points: Array.from({ length: days }, (_, d) => ({ day_ms: d * 86_400_000, low_centavos: 99_900, high_centavos: 99_900 })),
    })),
  };
  return { stats, history: series } as unknown as AgentPriceHistoryResponse;
}

describe("fitPriceHistory", () => {
  it("keeps short histories whole, stats first", () => {
    const out = fitPriceHistory(history(10, 2), 16_000);
    expect(out.startsWith('{"stats":')).toBe(true);
    expect(JSON.parse(out).history.listings[0].points).toHaveLength(10);
    expect(out).not.toContain("history_trimmed");
  });

  it("a year of history for three listings still yields valid JSON with every stat", () => {
    const out = fitPriceHistory(history(365, 3), 16_000);
    expect(out.length).toBeLessThanOrEqual(16_000);
    const parsed = JSON.parse(out);
    expect(parsed.history_trimmed).toBe(true);
    expect(parsed.stats.listings).toHaveLength(3);
    expect(parsed.stats.listings[0].windows[0].low_centavos).toBe(94_900);
    const points = parsed.history.listings[0].points;
    expect(points.length).toBeGreaterThan(0);
    expect(points.at(-1).day_ms).toBe(364 * 86_400_000);
  });
});
