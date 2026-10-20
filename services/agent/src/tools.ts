/**
 * The four assistant tools. Each is a zod schema plus one HTTP call to the
 * Rust api; strict objects reject extra keys, so a model cannot smuggle a
 * tenant or user id in.
 */
import type { AgentTool, ToolContext } from "@kenkaiiii/gg-agent";
import { z } from "zod";

import { ApiError, type AgentPriceHistoryResponse, type ApiClient } from "./api-client.ts";

const ASIN = z.string().regex(/^[A-Z0-9]{10}$/, "10 uppercase letters or digits");
const CENTAVOS = z.number().int().min(0).max(1_000_000_000_00);

export const searchOffersParams = z.strictObject({
  query: z.string().trim().min(1).max(100).optional().describe("Words that must appear in the title."),
  match_status: z
    .enum(["accepted", "needs_review"])
    .optional()
    .describe("Only listings whose Amazon match has this status."),
  limit: z.number().int().min(1).max(20).default(10),
});

export const priceHistoryParams = z.strictObject({
  asin: ASIN.describe("Amazon product, e.g. B0E2ETEST1."),
  days: z.number().int().min(1).max(365).default(365).describe("Days back from today."),
});

const roiItem = z.strictObject({
  buy_price_centavos: CENTAVOS,
  sell_price_centavos: CENTAVOS,
  amazon_fees_centavos: CENTAVOS,
  shipping_centavos: CENTAVOS.optional(),
  prep_fee_centavos: CENTAVOS.optional(),
  monthly_fixed_centavos: CENTAVOS.optional(),
  units_per_month: z.number().int().min(1).max(1_000_000).optional(),
});

export const computeRoiParams = z.strictObject({
  items: z.array(roiItem).min(1).max(20),
});

export const resolveMatchParams = z.strictObject({
  // Same bounds as domain::RetailerId / domain::RetailerSku, so the api never 422s.
  retailer: z.string().regex(/^[a-z0-9_]{1,40}$/, "lowercase letters, digits or _ (1-40)"),
  retailer_sku: z.string().min(1).max(100),
  asin: ASIN,
  decision: z.enum(["accept", "reject"]),
  reason: z.string().trim().min(1).max(300),
});

/** Bounds a tool's HTTP call by the loop's signal and the tool timeout. */
function callSignal(ctx: ToolContext, timeoutMs: number): AbortSignal {
  return AbortSignal.any([ctx.signal, AbortSignal.timeout(timeoutMs)]);
}

/**
 * Serializes a price history so the stats always survive the tool-result cap:
 * `stats` goes first, and when the JSON is still too long each listing keeps
 * only its newest points, halving until it fits (`history_trimmed: true`).
 */
export function fitPriceHistory(data: AgentPriceHistoryResponse, maxChars: number): string {
  const full = JSON.stringify({ stats: data.stats, history: data.history });
  if (full.length <= maxChars) return full;
  let keep = Math.max(0, ...data.history.listings.map((l) => l.points.length));
  for (;;) {
    keep = Math.floor(keep / 2);
    const history = {
      ...data.history,
      listings: data.history.listings.map((l) => ({ ...l, points: keep === 0 ? [] : l.points.slice(-keep) })),
    };
    const out = JSON.stringify({ stats: data.stats, history_trimmed: true, history });
    if (out.length <= maxChars || keep === 0) return out;
  }
}

/** Turns an api error into a short, fixed message the model may read. */
async function guarded<T>(
  run: () => Promise<T>,
  serialize: (value: T) => string = (value) => JSON.stringify(value),
): Promise<string> {
  try {
    return serialize(await run());
  } catch (e) {
    if (e instanceof ApiError) throw new Error(`api_error: ${e.code}`);
    throw new Error("api_unreachable");
  }
}

export function buildTools(api: ApiClient, timeoutMs: number, maxResultChars: number): AgentTool[] {
  const searchOffers: AgentTool<typeof searchOffersParams> = {
    name: "search_offers",
    description:
      "List the seller's tracked retailer offers (from their searches in the last 90 days), newest first. " +
      "Read-only: never starts a new search. Prices are integer centavos (MXN).",
    parameters: searchOffersParams,
    timeoutMs,
    execute: (args, ctx) =>
      guarded(() =>
        api.offers(
          { query: args.query, match_status: args.match_status, limit: args.limit },
          callSignal(ctx, timeoutMs),
        ),
      ),
  };

  const priceHistory: AgentTool<typeof priceHistoryParams> = {
    name: "get_price_history",
    description:
      "Daily retailer prices for an Amazon product plus 30/90/365-day low, high and average per listing. " +
      "Use it for season prep (e.g. compare with last Hot Sale or Buen Fin lows). Prices are centavos.",
    parameters: priceHistoryParams,
    timeoutMs,
    execute: (args, ctx) =>
      guarded(
        () => api.priceHistory(args.asin, args.days, callSignal(ctx, timeoutMs)),
        (data: AgentPriceHistoryResponse) => fitPriceHistory(data, maxResultChars),
      ),
  };

  const computeRoi: AgentTool<typeof computeRoiParams> = {
    name: "compute_roi",
    description:
      "Profit, ROI and margin for up to 20 buy/sell scenarios. Money is centavos, ratios basis points.",
    parameters: computeRoiParams,
    timeoutMs,
    execute: (args, ctx) => guarded(() => api.roi(args.items, callSignal(ctx, timeoutMs))),
  };

  const resolveMatch: AgentTool<typeof resolveMatchParams> = {
    name: "resolve_match",
    description:
      "Propose accepting or rejecting a needs_review match between a retailer listing and an Amazon product. " +
      "This only creates a suggestion; the seller must confirm it in the app. Never say the match is resolved.",
    parameters: resolveMatchParams,
    timeoutMs,
    executionMode: "sequential",
    execute: (args, ctx) => guarded(() => api.suggest(args, callSignal(ctx, timeoutMs))),
  };

  return [searchOffers, priceHistory, computeRoi, resolveMatch] as AgentTool[];
}
