import { describe, expect, it } from "vitest";

import { ConfigError, loadConfig } from "../src/config.ts";
import { allowlistedFetch, EgressRefused } from "../src/egress.ts";
import { PROVIDER_KEY, SERVICE_TOKEN } from "./helpers.ts";

const base = {
  AGENT_SERVICE_TOKEN: SERVICE_TOKEN,
  AGENT_API_URL: "http://127.0.0.1:8080",
  AGENT_PROVIDER: "anthropic",
  AGENT_MODEL: "claude-test",
  AGENT_PROVIDER_API_KEY: PROVIDER_KEY,
};

function variable(env: Record<string, string>): string | undefined {
  try {
    loadConfig(env);
    return undefined;
  } catch (e) {
    expect(e).toBeInstanceOf(ConfigError);
    expect(String(e)).not.toContain(PROVIDER_KEY);
    expect(String(e)).not.toContain(SERVICE_TOKEN);
    return (e as ConfigError).variable;
  }
}

describe("loadConfig", () => {
  it("defaults", () => {
    const c = loadConfig(base);
    expect(c.host).toBe("127.0.0.1");
    expect(c.port).toBe(8090);
    expect(c.limits).toEqual({
      maxTurns: 6,
      toolTimeoutMs: 10_000,
      runTimeoutMs: 60_000,
      maxOutputTokens: 1024,
      maxToolResultChars: 16_000,
    });
    expect(c.providerHosts).toEqual(["api.anthropic.com"]);
    expect(c.apiKey()).toBe(PROVIDER_KEY);
    expect(JSON.stringify(c)).not.toContain(PROVIDER_KEY);
  });

  it("fails closed", () => {
    expect(variable({ ...base, AGENT_SERVICE_TOKEN: "short" })).toBe("AGENT_SERVICE_TOKEN");
    expect(variable({ ...base, AGENT_PROVIDER_API_KEY: "" })).toBe("AGENT_PROVIDER_API_KEY");
    expect(variable({ ...base, AGENT_PROVIDER: "gemini" })).toBe("AGENT_PROVIDER");
    expect(variable({ ...base, AGENT_API_URL: "ftp://x" })).toBe("AGENT_API_URL");
    expect(variable({ ...base, AGENT_API_URL: "http://u:p@x" })).toBe("AGENT_API_URL");
    expect(variable({ ...base, AGENT_MAX_TURNS: "0" })).toBe("AGENT_MAX_TURNS");
    expect(variable({ ...base, AGENT_MAX_TURNS: "6x" })).toBe("AGENT_MAX_TURNS");
    expect(variable({ ...base, AGENT_ADDR: "nope" })).toBe("AGENT_ADDR");
    expect(variable({ ...base, GGAI_DUMP_REQUEST: "1" })).toBe("GGAI_DUMP_REQUEST");
  });

  it("the fake provider needs an explicit opt-in and never runs in production", () => {
    const fake = { ...base, AGENT_PROVIDER: "palsu", AGENT_PROVIDER_API_KEY: "" };
    expect(variable(fake)).toBe("AGENT_ALLOW_FAKE_PROVIDER");
    expect(variable({ ...fake, AGENT_ALLOW_FAKE_PROVIDER: "true", NODE_ENV: "production" })).toBe(
      "AGENT_ALLOW_FAKE_PROVIDER",
    );
    const c = loadConfig({ ...fake, AGENT_ALLOW_FAKE_PROVIDER: "true" });
    expect(c.provider).toBe("palsu");
    expect(c.providerHosts).toEqual([]);
  });
});

describe("allowlistedFetch", () => {
  const seen: string[] = [];
  const inner = (async (input: string | URL | Request) => {
    seen.push(String(input));
    return new Response("{}");
  }) as typeof fetch;
  const guarded = allowlistedFetch(["api.anthropic.com"], inner);

  it("allows only https to listed hosts on the default port", async () => {
    await guarded("https://api.anthropic.com/v1/messages");
    for (const url of [
      "http://api.anthropic.com/v1/messages",
      "https://api.anthropic.com:8443/v1",
      "https://evil.example/v1",
      "https://user:pw@api.anthropic.com/v1",
      "https://169.254.169.254/latest/meta-data",
      "not a url",
    ]) {
      await expect(guarded(url)).rejects.toBeInstanceOf(EgressRefused);
    }
    expect(seen).toEqual(["https://api.anthropic.com/v1/messages"]);
  });

  it("the fake provider has no egress at all", async () => {
    await expect(allowlistedFetch([], inner)("https://api.anthropic.com/")).rejects.toBeInstanceOf(EgressRefused);
  });
});
