import { palsuText, palsuToolCall, registerPalsuProvider, type Message } from "@kenkaiiii/gg-ai";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it } from "vitest";

import { chatRequestSchema, runChat, RunFailed, RunTimeout, type ChatRequest } from "../src/run.ts";
import { resolveMatchParams, searchOffersParams } from "../src/tools.ts";
import {
  chatBody,
  GRANT,
  json,
  PROVIDER_KEY,
  SERVICE_TOKEN,
  startFakeApi,
  testConfig,
  type FakeApi,
} from "./helpers.ts";

const palsu = registerPalsuProvider();
let api: FakeApi;

beforeAll(async () => {
  api = await startFakeApi();
});
afterAll(async () => {
  palsu.unregister();
  await api.close();
});
beforeEach(() => {
  palsu.setResponses([]);
  api.requests.length = 0;
  api.routes.clear();
});
afterEach(() => {
  expect(palsu.getPendingResponseCount()).toBe(0);
});

function request(content = "hola", extra: Record<string, unknown> = {}): ChatRequest {
  return chatRequestSchema.parse(chatBody(content, extra));
}

function toolRequests() {
  return api.requests.filter((r) => r.path !== "/internal/agent/usage");
}

/** The tool result the model saw on its last call. */
function lastToolResult(messages: Message[]): { content: string; isError: boolean } {
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i];
    if (m?.role === "tool") {
      const r = m.content.at(-1);
      const content = typeof r?.content === "string" ? r.content : JSON.stringify(r?.content);
      return { content, isError: r?.isError === true };
    }
  }
  throw new Error("no tool result");
}

describe("tools call the api with bearer + grant", () => {
  it("search_offers", async () => {
    palsu.setResponses([palsuToolCall("search_offers", { query: "cafetera", match_status: "needs_review", limit: 5 }), palsuText("ok")]);
    const reply = await runChat(testConfig(api.url), request(), new AbortController().signal);
    expect(reply?.reply).toBe("ok");
    const [call] = toolRequests();
    expect(call?.method).toBe("GET");
    expect(call?.path).toBe("/internal/agent/offers");
    expect(Object.fromEntries(call?.query ?? [])).toEqual({ query: "cafetera", match_status: "needs_review", limit: "5" });
    expect(call?.headers.authorization).toBe(`Bearer ${SERVICE_TOKEN}`);
    expect(call?.headers["x-agent-grant"]).toBe(GRANT);
  });

  it("get_price_history", async () => {
    api.routes.set("GET /internal/agent/products/B0E2ETEST1/price-history", (_r, res) =>
      json(res, 200, { stats: { asin: "B0E2ETEST1", listings: [] }, series: [] }),
    );
    palsu.setResponses([palsuToolCall("get_price_history", { asin: "B0E2ETEST1", days: 90 }), palsuText("ok")]);
    await runChat(testConfig(api.url), request(), new AbortController().signal);
    const [call] = toolRequests();
    expect(call?.path).toBe("/internal/agent/products/B0E2ETEST1/price-history");
    expect(call?.query.get("days")).toBe("90");
    expect(call?.headers["x-agent-grant"]).toBe(GRANT);
  });

  it("compute_roi", async () => {
    api.routes.set("POST /internal/agent/roi", (_r, res) => json(res, 200, { items: [] }));
    const items = [{ buy_price_centavos: 10000, sell_price_centavos: 20000, amazon_fees_centavos: 3000 }];
    palsu.setResponses([palsuToolCall("compute_roi", { items }), palsuText("ok")]);
    await runChat(testConfig(api.url), request(), new AbortController().signal);
    const [call] = toolRequests();
    expect(call?.method).toBe("POST");
    expect(call?.path).toBe("/internal/agent/roi");
    expect(call?.body).toEqual({ items });
    expect(call?.headers.authorization).toBe(`Bearer ${SERVICE_TOKEN}`);
  });

  it("resolve_match only posts a suggestion", async () => {
    api.routes.set("POST /internal/agent/match-suggestions", (_r, res) => json(res, 201, { id: "s1" }));
    const args = { retailer: "costco_mx", retailer_sku: "E2E-COS-1", asin: "B0E2ETEST1", decision: "accept", reason: "misma marca" };
    palsu.setResponses([palsuToolCall("resolve_match", args), palsuText("ok")]);
    await runChat(testConfig(api.url), request(), new AbortController().signal);
    const [call] = toolRequests();
    expect(call?.path).toBe("/internal/agent/match-suggestions");
    expect(call?.body).toEqual(args);
  });

  it("tool args can never carry a tenant", async () => {
    expect(searchOffersParams.safeParse({ tenant: "tenant-b" }).success).toBe(false);
    expect(
      resolveMatchParams.safeParse({
        retailer: "costco_mx",
        retailer_sku: "X",
        asin: "B0E2ETEST1",
        decision: "accept",
        reason: "r",
        tenant: "tenant-b",
      }).success,
    ).toBe(false);
    palsu.setResponses([palsuToolCall("search_offers", { tenant: "tenant-b", limit: 5 }), palsuText("ok")]);
    await runChat(testConfig(api.url), request(), new AbortController().signal);
    expect(toolRequests()).toHaveLength(0);
  });

  it("api errors reach the model as a fixed code", async () => {
    api.routes.set("POST /internal/agent/match-suggestions", (_r, res) => json(res, 422, { error: "not_reviewable" }));
    let seen: { content: string; isError: boolean } | undefined;
    palsu.setResponses([
      palsuToolCall("resolve_match", { retailer: "r", retailer_sku: "s", asin: "B0E2ETEST1", decision: "reject", reason: "x" }),
      (messages) => {
        seen = lastToolResult(messages);
        return palsuText("ok");
      },
    ]);
    await runChat(testConfig(api.url), request(), new AbortController().signal);
    expect(seen?.isError).toBe(true);
    expect(seen?.content).toContain("api_error: not_reviewable");
  });
});

describe("limits", () => {
  it("stops at maxTurns", async () => {
    palsu.setResponses([
      palsuToolCall("search_offers", { limit: 1 }),
      palsuToolCall("search_offers", { limit: 1 }),
      palsuToolCall("search_offers", { limit: 1 }),
    ]);
    const reply = await runChat(testConfig(api.url, { maxTurns: 2 }), request(), new AbortController().signal);
    palsu.setResponses([]);
    expect(reply?.stop).toBe("max_turns");
    expect(palsu.state.callCount).toBeGreaterThan(0);
    expect(toolRequests().length).toBeLessThanOrEqual(2);
  });

  it("times out a slow tool and aborts its http call", async () => {
    api.routes.set("GET /internal/agent/offers", () => {
      // never answers
    });
    let seen: { content: string; isError: boolean } | undefined;
    palsu.setResponses([
      palsuToolCall("search_offers", { limit: 1 }),
      (messages) => {
        seen = lastToolResult(messages);
        return palsuText("sin datos");
      },
    ]);
    const reply = await runChat(testConfig(api.url, { toolTimeoutMs: 200 }), request(), new AbortController().signal);
    expect(reply?.reply).toBe("sin datos");
    expect(seen?.isError).toBe(true);
    const [call] = toolRequests();
    expect(await call?.aborted).toBe(true);
  });

  it("aborts the run and the in-flight tool call when the client goes away", async () => {
    api.routes.set("GET /internal/agent/offers", () => {
      // never answers
    });
    palsu.setResponses([palsuToolCall("search_offers", { limit: 1 }), palsuText("never")]);
    const outer = new AbortController();
    const run = runChat(testConfig(api.url, { toolTimeoutMs: 30_000 }), request(), outer.signal);
    while (toolRequests().length === 0) await new Promise((r) => setTimeout(r, 10));
    outer.abort();
    expect(await run).toBeNull();
    expect(await toolRequests()[0]?.aborted).toBe(true);
    palsu.setResponses([]);
  });

  it("whole-run timeout", async () => {
    api.routes.set("GET /internal/agent/offers", () => {
      // never answers
    });
    palsu.setResponses([palsuToolCall("search_offers", { limit: 1 }), palsuText("never")]);
    await expect(
      runChat(testConfig(api.url, { toolTimeoutMs: 30_000, runTimeoutMs: 300 }), request(), new AbortController().signal),
    ).rejects.toBeInstanceOf(RunTimeout);
    palsu.setResponses([]);
  });

  it("stops before the next model call once the local budget is spent", async () => {
    // palsu reports 100 input tokens per call.
    palsu.setResponses([palsuToolCall("search_offers", { limit: 1 }), palsuText("never")]);
    const reply = await runChat(testConfig(api.url), request("hola", { token_budget: 50 }), new AbortController().signal);
    palsu.setResponses([]);
    expect(reply?.stop).toBe("token_budget");
    expect(palsu.state.callCount).toBeGreaterThan(0);
    const usage = api.requests.filter((r) => r.path === "/internal/agent/usage");
    expect(usage).toHaveLength(1);
    expect((usage[0]?.body as { input_tokens: number }).input_tokens).toBe(100);
  });

  it("stops when the api says the tenant's budget is gone", async () => {
    api.routes.set("POST /internal/agent/usage", (_r, res) => json(res, 200, { remaining_tokens: 0 }));
    palsu.setResponses([palsuToolCall("search_offers", { limit: 1 }), palsuText("never")]);
    const reply = await runChat(testConfig(api.url), request(), new AbortController().signal);
    palsu.setResponses([]);
    expect(reply?.stop).toBe("token_budget");
  });

  it("keeps a final answer that crosses the budget", async () => {
    palsu.setResponses([palsuText("respuesta")]);
    const reply = await runChat(testConfig(api.url), request("hola", { token_budget: 50 }), new AbortController().signal);
    expect(reply).toMatchObject({ reply: "respuesta", stop: "done" });
  });

  it("fails closed when usage cannot be recorded", async () => {
    api.routes.set("POST /internal/agent/usage", (_r, res) => json(res, 503, { error: "unavailable" }));
    palsu.setResponses([palsuToolCall("search_offers", { limit: 1 }), palsuText("never")]);
    await expect(runChat(testConfig(api.url), request(), new AbortController().signal)).rejects.toBeInstanceOf(RunFailed);
    palsu.setResponses([]);
  });
});

describe("secrets", () => {
  it("a provider error echoing the key is reduced to RunFailed", async () => {
    palsu.setResponses([
      (_m, options) => {
        throw new Error(`FAKE_PROVIDER_FAILURE 401 invalid x-api-key ${String(options.apiKey ?? PROVIDER_KEY)}`);
      },
    ]);
    const err = await runChat(testConfig(api.url), request(), new AbortController().signal).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(RunFailed);
    expect(String(err)).not.toContain(PROVIDER_KEY);
    expect(JSON.stringify(err)).not.toContain(PROVIDER_KEY);
  });
});
