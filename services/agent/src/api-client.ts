/**
 * Typed calls to the Rust api's `/internal/agent/*` routes. Every request
 * carries the service bearer and the chat's grant; the tenant comes only from
 * the grant, so nothing here takes one.
 */
import type { components } from "./api/schema.ts";

type Schemas = components["schemas"];
export type AgentOffersResponse = Schemas["AgentOffersResponse"];
export type AgentPriceHistoryResponse = Schemas["AgentPriceHistoryResponse"];
export type AgentSuggestionRequest = Schemas["AgentSuggestionRequest"];
export type MatchSuggestionResponse = Schemas["MatchSuggestionResponse"];
export type RoiItemRequest = Schemas["RoiItemRequest"];
export type RoiResponse = Schemas["RoiResponse"];
export type AgentUsageResponse = Schemas["AgentUsageResponse"];

/** Largest api response body read, in bytes. */
export const MAX_API_RESPONSE_BYTES = 256 * 1024;

/** The api answered with an error code (a fixed, machine-readable string). */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string) {
    super(`api error ${status} ${code}`);
    this.status = status;
    this.code = code;
    this.name = "ApiError";
  }
}

const CODE = /^[a-z_]{1,40}$/;

async function readBounded(response: Response, limit: number): Promise<string> {
  const reader = response.body?.getReader();
  if (!reader) return "";
  const chunks: Uint8Array[] = [];
  let size = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    size += value.byteLength;
    if (size > limit) {
      await reader.cancel();
      throw new ApiError(response.status, "response_too_large");
    }
    chunks.push(value);
  }
  return new TextDecoder().decode(Buffer.concat(chunks));
}

export interface ApiClientOptions {
  baseUrl: URL;
  serviceToken: string;
  grant: string;
  fetch?: typeof fetch;
}

export class ApiClient {
  readonly #base: URL;
  readonly #headers: Record<string, string>;
  readonly #fetch: typeof fetch;

  constructor(options: ApiClientOptions) {
    this.#base = options.baseUrl;
    this.#headers = {
      authorization: `Bearer ${options.serviceToken}`,
      "x-agent-grant": options.grant,
      accept: "application/json",
    };
    this.#fetch = options.fetch ?? globalThis.fetch;
  }

  async #call<T>(method: "GET" | "POST", path: string, signal: AbortSignal, body?: unknown): Promise<T> {
    const url = new URL(path, this.#base);
    const init: RequestInit = {
      method,
      headers: body === undefined ? this.#headers : { ...this.#headers, "content-type": "application/json" },
      signal,
      redirect: "error",
    };
    if (body !== undefined) init.body = JSON.stringify(body);
    const response = await this.#fetch(url, init);
    const text = await readBounded(response, MAX_API_RESPONSE_BYTES);
    if (!response.ok) {
      let code = "api_error";
      try {
        const parsed: unknown = JSON.parse(text);
        if (parsed && typeof parsed === "object" && "error" in parsed) {
          const raw = (parsed as { error: unknown }).error;
          if (typeof raw === "string" && CODE.test(raw)) code = raw;
        }
      } catch {
        // keep the generic code
      }
      throw new ApiError(response.status, code);
    }
    try {
      return JSON.parse(text) as T;
    } catch {
      throw new ApiError(response.status, "invalid_response");
    }
  }

  offers(
    params: { query?: string | undefined; match_status?: string | undefined; limit: number },
    signal: AbortSignal,
  ): Promise<AgentOffersResponse> {
    const q = new URLSearchParams();
    if (params.query) q.set("query", params.query);
    if (params.match_status) q.set("match_status", params.match_status);
    q.set("limit", String(params.limit));
    return this.#call("GET", `/internal/agent/offers?${q.toString()}`, signal);
  }

  priceHistory(asin: string, days: number, signal: AbortSignal): Promise<AgentPriceHistoryResponse> {
    const q = new URLSearchParams({ days: String(days) });
    return this.#call(
      "GET",
      `/internal/agent/products/${encodeURIComponent(asin)}/price-history?${q.toString()}`,
      signal,
    );
  }

  roi(items: RoiItemRequest[], signal: AbortSignal): Promise<RoiResponse> {
    return this.#call("POST", "/internal/agent/roi", signal, { items });
  }

  suggest(body: AgentSuggestionRequest, signal: AbortSignal): Promise<MatchSuggestionResponse> {
    return this.#call("POST", "/internal/agent/match-suggestions", signal, body);
  }

  usage(input_tokens: number, output_tokens: number, signal: AbortSignal): Promise<AgentUsageResponse> {
    return this.#call("POST", "/internal/agent/usage", signal, { input_tokens, output_tokens });
  }
}
