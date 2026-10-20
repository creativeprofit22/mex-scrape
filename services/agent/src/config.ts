/**
 * Environment parsing. Fails closed: a missing or malformed variable stops
 * the process with an error naming the variable, never its value.
 */

/** Model providers the sidecar may talk to. `palsu` is the scripted fake. */
export const PROVIDERS = ["anthropic", "openai", "palsu"] as const;
export type ProviderName = (typeof PROVIDERS)[number];

/** Hosts each real provider's SDK calls (the egress allowlist). */
const PROVIDER_HOSTS: Record<Exclude<ProviderName, "palsu">, readonly string[]> = {
  anthropic: ["api.anthropic.com"],
  openai: ["api.openai.com"],
};

const MIN_SERVICE_TOKEN_CHARS = 32;

export interface Limits {
  /** Model turns per chat (`maxTurns`); extensions are disabled. */
  maxTurns: number;
  /** Per-tool timeout, also bounding the tool's HTTP call. */
  toolTimeoutMs: number;
  /** Whole-run timeout. */
  runTimeoutMs: number;
  /** Output tokens per model turn (`maxTokens`). */
  maxOutputTokens: number;
  /** Characters of one tool result the model sees. */
  maxToolResultChars: number;
}

export interface Config {
  host: string;
  port: number;
  /** Bearer the Rust api must send; also sent back on tool calls. */
  serviceToken: string;
  /** Base URL of the Rust api, e.g. `http://127.0.0.1:8080`. */
  apiUrl: URL;
  provider: ProviderName;
  model: string;
  /** Hosts the provider transport may reach (empty for the fake). */
  providerHosts: readonly string[];
  limits: Limits;
  /** The provider API key, held in a closure so it is never serialized. */
  apiKey: () => string | undefined;
}

export class ConfigError extends Error {
  readonly variable: string;

  constructor(variable: string) {
    super(`invalid or missing ${variable}`);
    this.variable = variable;
    this.name = "ConfigError";
  }
}

type Env = Readonly<Record<string, string | undefined>>;

function required(env: Env, name: string): string {
  const value = env[name]?.trim();
  if (!value) throw new ConfigError(name);
  return value;
}

function intVar(env: Env, name: string, fallback: number, min: number, max: number): number {
  const raw = env[name]?.trim();
  if (raw === undefined || raw === "") return fallback;
  if (!/^\d+$/.test(raw)) throw new ConfigError(name);
  const n = Number(raw);
  if (!Number.isSafeInteger(n) || n < min || n > max) throw new ConfigError(name);
  return n;
}

function parseAddr(raw: string): { host: string; port: number } {
  const at = raw.lastIndexOf(":");
  if (at <= 0) throw new ConfigError("AGENT_ADDR");
  const host = raw.slice(0, at).replace(/^\[(.*)\]$/, "$1");
  const portText = raw.slice(at + 1);
  if (!/^\d+$/.test(portText)) throw new ConfigError("AGENT_ADDR");
  const port = Number(portText);
  if (port < 1 || port > 65535) throw new ConfigError("AGENT_ADDR");
  return { host, port };
}

/** Reads the sidecar configuration from `env`. */
export function loadConfig(env: Env): Config {
  // gg-ai writes request dumps to /tmp when this is set; never allowed here.
  if (env.GGAI_DUMP_REQUEST) throw new ConfigError("GGAI_DUMP_REQUEST");

  const { host, port } = parseAddr(env.AGENT_ADDR?.trim() || "127.0.0.1:8090");

  const serviceToken = required(env, "AGENT_SERVICE_TOKEN");
  if (serviceToken.length < MIN_SERVICE_TOKEN_CHARS) throw new ConfigError("AGENT_SERVICE_TOKEN");

  let apiUrl: URL;
  try {
    apiUrl = new URL(required(env, "AGENT_API_URL"));
  } catch (e) {
    if (e instanceof ConfigError) throw e;
    throw new ConfigError("AGENT_API_URL");
  }
  if (!["http:", "https:"].includes(apiUrl.protocol) || apiUrl.username || apiUrl.password) {
    throw new ConfigError("AGENT_API_URL");
  }

  const providerRaw = required(env, "AGENT_PROVIDER");
  if (!(PROVIDERS as readonly string[]).includes(providerRaw)) throw new ConfigError("AGENT_PROVIDER");
  const provider = providerRaw as ProviderName;

  let key: string | undefined;
  let providerHosts: readonly string[] = [];
  if (provider === "palsu") {
    if (env.AGENT_ALLOW_FAKE_PROVIDER !== "true" || env.NODE_ENV === "production") {
      throw new ConfigError("AGENT_ALLOW_FAKE_PROVIDER");
    }
  } else {
    key = required(env, "AGENT_PROVIDER_API_KEY");
    providerHosts = PROVIDER_HOSTS[provider];
  }
  const model = provider === "palsu" ? env.AGENT_MODEL?.trim() || "scripted" : required(env, "AGENT_MODEL");

  const limits: Limits = {
    maxTurns: intVar(env, "AGENT_MAX_TURNS", 6, 1, 20),
    toolTimeoutMs: intVar(env, "AGENT_TOOL_TIMEOUT_MS", 10_000, 100, 60_000),
    runTimeoutMs: intVar(env, "AGENT_RUN_TIMEOUT_MS", 60_000, 1_000, 70_000),
    maxOutputTokens: intVar(env, "AGENT_MAX_OUTPUT_TOKENS", 1024, 64, 8192),
    maxToolResultChars: intVar(env, "AGENT_MAX_TOOL_RESULT_CHARS", 16_000, 1_000, 64_000),
  };

  return {
    host,
    port,
    serviceToken,
    apiUrl,
    provider,
    model,
    providerHosts,
    limits,
    apiKey: () => key,
  };
}
