/**
 * Runs the real entry point as a child process with an empty HOME and the
 * scripted fake model, then checks: the use cases work end to end, the home
 * directory stays empty, no CLI/TUI module is loaded and no secret is printed.
 */
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtempSync, readdirSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

import { afterAll, beforeAll, describe, expect, it } from "vitest";

import { chatBody, json, postChat, SERVICE_TOKEN, startFakeApi, until, type FakeApi } from "./helpers.ts";

const ROOT = fileURLToPath(new URL("..", import.meta.url));
const PORT = 18_000 + (process.pid % 1000);
const base = new URL(`http://127.0.0.1:${PORT}`);

let api: FakeApi;
let home: string;
let child: ChildProcess;
let stdout = "";
let stderr = "";

beforeAll(async () => {
  api = await startFakeApi();
  api.routes.set("GET /internal/agent/products/B0E2ETEST1/price-history", (_r, res) =>
    json(res, 200, {
      stats: {
        asin: "B0E2ETEST1",
        as_of_ms: 0,
        hidden_listings: 0,
        listings: [
          {
            retailer: "costco_mx",
            retailer_sku: "E2E-COS-1",
            windows: [
              { days: 30, low_centavos: 149_900, high_centavos: 159_900, average_centavos: 154_900, observations: 3 },
              { days: 365, low_centavos: 123_450, high_centavos: 159_900, average_centavos: 140_000, observations: 9 },
            ],
          },
        ],
      },
      history: { asin: "B0E2ETEST1", days: 365, series: [] },
    }),
  );
  api.routes.set("GET /internal/agent/offers", (_r, res) =>
    json(res, 200, {
      hidden: 0,
      offers: [
        {
          search_id: 1,
          seen_at_ms: 0,
          offer: {
            retailer: "costco_mx",
            retailer_sku: "E2E-COS-1",
            title: "Cafetera E2E",
            url: "https://example.test/p",
            price_centavos: 149_900,
            availability: "in_stock",
            membership_required: false,
            amazon: { asin: "B0E2ETEST1", match_status: "needs_review", confidence_bp: 6000 },
          },
        },
      ],
    }),
  );
  api.routes.set("POST /internal/agent/match-suggestions", (_r, res) => json(res, 201, { id: 7 }));

  home = mkdtempSync(join(tmpdir(), "agent-home-"));
  const env: Record<string, string> = {
    PATH: process.env.PATH ?? "",
    SystemRoot: process.env.SystemRoot ?? "",
    HOME: home,
    USERPROFILE: home,
    APPDATA: join(home, "AppData"),
    LOCALAPPDATA: join(home, "LocalAppData"),
    XDG_CONFIG_HOME: join(home, ".config"),
    AGENT_ADDR: `127.0.0.1:${PORT}`,
    AGENT_SERVICE_TOKEN: SERVICE_TOKEN,
    AGENT_API_URL: api.url.toString(),
    AGENT_PROVIDER: "palsu",
    AGENT_ALLOW_FAKE_PROVIDER: "true",
  };
  child = spawn(process.execPath, ["--import", "./test/support/trace-imports.mjs", "src/main.ts"], {
    cwd: ROOT,
    env,
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout?.on("data", (c: Buffer) => (stdout += c.toString()));
  child.stderr?.on("data", (c: Buffer) => (stderr += c.toString()));
  await until(() => stdout.includes('"msg":"listening"') || child.exitCode !== null, 15_000);
  expect(child.exitCode).toBeNull();
}, 30_000);

afterAll(async () => {
  child?.kill();
  await api?.close();
  rmSync(home, { recursive: true, force: true });
});

describe("headless sidecar", () => {
  it("season prep answer is grounded in the price-history tool result", async () => {
    const res = await postChat(base, chatBody("¿Cuándo conviene comprar B0E2ETEST1 para el Buen Fin?"));
    expect(res.status).toBe(200);
    const body = (await res.json()) as { reply: string; stop: string };
    expect(body.stop).toBe("done");
    expect(body.reply).toContain("$1,234.50");
    expect(api.requests.some((r) => r.path.endsWith("/B0E2ETEST1/price-history"))).toBe(true);
  });

  it("tie-break only proposes, through the api", async () => {
    const res = await postChat(base, chatBody("Ayúdame a desempatar las coincidencias por revisar", { chat_id: "chat-2" }));
    expect(res.status).toBe(200);
    const body = (await res.json()) as { reply: string };
    expect(body.reply).toMatch(/Propuse/);
    const posted = api.requests.find((r) => r.path === "/internal/agent/match-suggestions");
    expect(posted?.body).toMatchObject({
      retailer: "costco_mx",
      retailer_sku: "E2E-COS-1",
      asin: "B0E2ETEST1",
      decision: "accept",
    });
  });

  it("leaves HOME empty, loads no CLI/TUI code and prints no secrets", () => {
    expect(readdirSync(home)).toEqual([]);

    const imports = stderr
      .split("\n")
      .filter((l) => l.startsWith("IMPORT "))
      .map((l) => l.slice("IMPORT ".length));
    expect(imports.some((u) => u.includes("/@kenkaiiii/gg-agent/"))).toBe(true);
    const kenkaiii = new Set(
      imports.flatMap((u) => {
        const m = /\/@kenkaiiii\/([^/]+)\//.exec(u);
        return m?.[1] ? [m[1]] : [];
      }),
    );
    expect([...kenkaiii].sort()).toEqual(["gg-agent", "gg-ai"]);
    const banned = /\/node_modules\/(ink|react|commander|yargs|blessed|@inquirer|inquirer|ggcoder|@kenkaiiii\/gg-core)\//;
    expect(imports.filter((u) => banned.test(u))).toEqual([]);

    const output = stdout + stderr.split("\n").filter((l) => !l.startsWith("IMPORT ")).join("\n");
    expect(output).not.toContain(SERVICE_TOKEN);
    for (const line of stdout.trim().split("\n")) expect(() => JSON.parse(line) as unknown).not.toThrow();
  });
});
