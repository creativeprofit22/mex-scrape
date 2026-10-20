import type { Server } from "node:http";

import { palsuText, palsuToolCall, registerPalsuProvider } from "@kenkaiiii/gg-ai";
import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { createAgentServer, installProcessGuards } from "../src/server.ts";
import {
  chatBody,
  listen,
  postChat,
  PROVIDER_KEY,
  SERVICE_TOKEN,
  startFakeApi,
  testConfig,
  until,
  type FakeApi,
} from "./helpers.ts";

const palsu = registerPalsuProvider();
let api: FakeApi;
let server: Server;
let base: URL;
const logs: string[] = [];

beforeAll(async () => {
  api = await startFakeApi();
  server = createAgentServer(testConfig(api.url, { toolTimeoutMs: 30_000 }), {
    log: (msg, fields) => logs.push(JSON.stringify({ msg, ...fields })),
  });
  base = await listen(server);
});
afterAll(async () => {
  palsu.unregister();
  server.closeAllConnections();
  await new Promise<void>((r) => server.close(() => r()));
  await api.close();
});
beforeEach(() => {
  palsu.setResponses([]);
  api.requests.length = 0;
  api.routes.clear();
  logs.length = 0;
});
afterEach(() => {
  vi.restoreAllMocks();
});

const hang = () => {
  // never answers
};

describe("auth and validation", () => {
  it("rejects a missing or wrong service token", async () => {
    expect((await postChat(base, chatBody("hola"), { token: null })).status).toBe(401);
    expect((await postChat(base, chatBody("hola"), { token: "x".repeat(40) })).status).toBe(401);
    expect(palsu.state.callCount).toBe(0);
  });

  it("rejects malformed bodies", async () => {
    expect((await postChat(base, { ...chatBody("hola"), messages: [] })).status).toBe(422);
    expect(
      (await postChat(base, { ...chatBody("hola"), messages: [{ role: "assistant", content: "x" }] })).status,
    ).toBe(422);
    const big = await postChat(base, chatBody("x".repeat(70 * 1024)));
    expect(big.status).toBe(413);
  });

  it("serves a chat", async () => {
    palsu.setResponses([palsuText("hola, ¿en qué te ayudo?")]);
    const res = await postChat(base, chatBody("hola"));
    expect(res.status).toBe(200);
    expect(await res.json()).toMatchObject({ reply: "hola, ¿en qué te ayudo?", stop: "done" });
  });
});

describe("concurrency and disconnect", () => {
  it("one in-flight chat per tenant", async () => {
    api.routes.set("GET /internal/agent/offers", hang);
    palsu.setResponses([palsuToolCall("search_offers", { limit: 1 }), palsuText("never")]);
    const client = new AbortController();
    const first = postChat(base, chatBody("hola"), { signal: client.signal }).catch(() => undefined);
    await until(() => api.requests.some((r) => r.path === "/internal/agent/offers"));

    const busy = await postChat(base, chatBody("otra"));
    expect(busy.status).toBe(409);
    expect(await busy.json()).toEqual({ error: "agent_busy" });

    // Another tenant is not blocked.
    palsu.setResponses([palsuText("ok")]);
    const other = await postChat(base, chatBody("hola", { tenant: "tenant-b" }));
    expect(other.status).toBe(200);

    client.abort();
    await first;
    await until(() => logs.some((l) => l.includes("chat_aborted")));
    palsu.setResponses([]);
  });

  it("client disconnect aborts the run and the api call", async () => {
    api.routes.set("GET /internal/agent/offers", hang);
    palsu.setResponses([palsuToolCall("search_offers", { limit: 1 }), palsuText("never")]);
    const client = new AbortController();
    const pending = postChat(base, chatBody("hola"), { signal: client.signal }).catch(() => "aborted");
    await until(() => api.requests.some((r) => r.path === "/internal/agent/offers"));
    client.abort();
    expect(await pending).toBe("aborted");

    const call = api.requests.find((r) => r.path === "/internal/agent/offers");
    expect(await call?.aborted).toBe(true);
    await until(() => logs.some((l) => l.includes("chat_aborted")));
    palsu.setResponses([]);

    // The tenant is free again.
    palsu.setResponses([palsuText("ok")]);
    expect((await postChat(base, chatBody("hola"))).status).toBe(200);
  });
});

describe("secrets never leave the process", () => {
  it("unhandled rejections are logged as a fixed message only", () => {
    const seen: string[] = [];
    const before = process.listeners("unhandledRejection");
    installProcessGuards((msg, fields) => seen.push(JSON.stringify({ msg, ...fields })));
    const added = process.listeners("unhandledRejection").filter((l) => !before.includes(l));
    expect(added).toHaveLength(1);
    added[0]?.(new Error(`provider said ${PROVIDER_KEY}`), Promise.resolve());
    process.off("unhandledRejection", added[0] as NodeJS.UnhandledRejectionListener);
    expect(seen).toEqual([JSON.stringify({ msg: "unhandled_rejection" })]);
  });

  it("provider errors echoing the key", async () => {
    const out: string[] = [];
    vi.spyOn(process.stdout, "write").mockImplementation((chunk: string | Uint8Array) => {
      out.push(String(chunk));
      return true;
    });
    vi.spyOn(process.stderr, "write").mockImplementation((chunk: string | Uint8Array) => {
      out.push(String(chunk));
      return true;
    });
    const spies = (["log", "info", "warn", "error", "debug"] as const).map((m) =>
      vi.spyOn(console, m).mockImplementation((...args: unknown[]) => {
        out.push(args.map(String).join(" "));
      }),
    );

    palsu.setResponses([
      () => {
        throw new Error(`FAKE_PROVIDER_FAILURE 401 Unauthorized: invalid api key ${PROVIDER_KEY}`);
      },
    ]);
    const res = await postChat(base, chatBody("hola"));
    const text = await res.text();
    expect(res.status).toBe(502);
    expect(text).toBe(JSON.stringify({ error: "agent_failed" }));

    const everything = [text, ...out, ...logs].join("\n");
    expect(everything).not.toContain(PROVIDER_KEY);
    expect(everything).not.toContain(SERVICE_TOKEN);
    expect(logs.some((l) => l.includes("chat_failed"))).toBe(true);
    spies.forEach((s) => s.mockRestore());
  });
});
