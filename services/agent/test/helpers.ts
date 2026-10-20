/** Test fixtures: an in-process fake Rust api and a sidecar config. */
import { once } from "node:events";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";

import type { Config } from "../src/config.ts";

export const SERVICE_TOKEN = "test-service-token-0123456789abcdef0123";
export const GRANT = "test-grant.v1.signature";
export const PROVIDER_KEY = "sk-test-PROVIDER-KEY-must-never-leak-7f3a";

export interface RecordedRequest {
  method: string;
  path: string;
  query: URLSearchParams;
  headers: IncomingMessage["headers"];
  body: unknown;
  /** Resolves true when the client went away before a response was sent. */
  aborted: Promise<boolean>;
}

export type Handler = (req: RecordedRequest, res: ServerResponse) => void | Promise<void>;

export interface FakeApi {
  url: URL;
  requests: RecordedRequest[];
  /** Per-route handlers, keyed by `METHOD /path` (no query). */
  routes: Map<string, Handler>;
  close(): Promise<void>;
}

export function json(res: ServerResponse, status: number, body: unknown): void {
  const text = JSON.stringify(body);
  res.writeHead(status, { "content-type": "application/json" });
  res.end(text);
}

const DEFAULTS: Record<string, Handler> = {
  "POST /internal/agent/usage": (_req, res) => json(res, 200, { remaining_tokens: 1_000_000 }),
  "GET /internal/agent/offers": (_req, res) => json(res, 200, { offers: [], hidden: 0 }),
};

export async function startFakeApi(): Promise<FakeApi> {
  const requests: RecordedRequest[] = [];
  const routes = new Map<string, Handler>();
  const server: Server = createServer((req, res) => {
    const chunks: Buffer[] = [];
    req.on("data", (c: Buffer) => chunks.push(c));
    req.on("end", () => {
      const url = new URL(req.url ?? "/", "http://fake");
      const raw = Buffer.concat(chunks).toString("utf8");
      const aborted = new Promise<boolean>((resolve) => {
        res.on("close", () => resolve(!res.writableEnded));
      });
      const recorded: RecordedRequest = {
        method: req.method ?? "",
        path: url.pathname,
        query: url.searchParams,
        headers: req.headers,
        body: raw ? (JSON.parse(raw) as unknown) : undefined,
        aborted,
      };
      requests.push(recorded);
      const key = `${recorded.method} ${recorded.path}`;
      const handler = routes.get(key) ?? DEFAULTS[key];
      if (!handler) {
        json(res, 404, { error: "not_found" });
        return;
      }
      void handler(recorded, res);
    });
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const { port } = server.address() as AddressInfo;
  return {
    url: new URL(`http://127.0.0.1:${port}`),
    requests,
    routes,
    close: () =>
      new Promise((resolve) => {
        server.closeAllConnections();
        server.close(() => resolve());
      }),
  };
}

export function testConfig(apiUrl: URL, overrides: Partial<Config["limits"]> = {}): Config {
  return {
    host: "127.0.0.1",
    port: 0,
    serviceToken: SERVICE_TOKEN,
    apiUrl,
    provider: "palsu",
    model: "scripted",
    providerHosts: [],
    limits: {
      maxTurns: 6,
      toolTimeoutMs: 2_000,
      runTimeoutMs: 10_000,
      maxOutputTokens: 1024,
      maxToolResultChars: 16_000,
      ...overrides,
    },
    apiKey: () => PROVIDER_KEY,
  };
}

export function chatBody(content: string, extra: Record<string, unknown> = {}) {
  return {
    chat_id: "chat-1",
    tenant: "tenant-a",
    locale: "es",
    grant: GRANT,
    token_budget: 100_000,
    messages: [{ role: "user", content }],
    ...extra,
  };
}

export async function listen(server: Server): Promise<URL> {
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const { port } = server.address() as AddressInfo;
  return new URL(`http://127.0.0.1:${port}`);
}

export async function postChat(
  base: URL,
  body: unknown,
  options: { token?: string | null; signal?: AbortSignal } = {},
): Promise<Response> {
  const headers: Record<string, string> = { "content-type": "application/json" };
  const token = options.token === undefined ? SERVICE_TOKEN : options.token;
  if (token !== null) headers.authorization = `Bearer ${token}`;
  return fetch(new URL("/v1/chat", base), {
    method: "POST",
    headers,
    body: JSON.stringify(body),
    ...(options.signal ? { signal: options.signal } : {}),
  });
}

/** Waits until `predicate` holds, polling briefly. */
export async function until(predicate: () => boolean, timeoutMs = 5_000): Promise<void> {
  const start = Date.now();
  while (!predicate()) {
    if (Date.now() - start > timeoutMs) throw new Error("condition not met in time");
    await new Promise((r) => setTimeout(r, 10));
  }
}
