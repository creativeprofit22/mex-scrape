/**
 * One chat run: builds a fresh gg-agent `Agent`, consumes its event stream
 * and enforces the limits (turns, run timeout, client disconnect, the
 * tenant's daily token budget).
 */
import { Agent, type AgentOptions } from "@kenkaiiii/gg-agent";
import type { Message } from "@kenkaiiii/gg-ai";
import { z } from "zod";

import { ApiClient } from "./api-client.ts";
import type { Config } from "./config.ts";
import { allowlistedFetch } from "./egress.ts";
import { fixedMessage, systemPrompt, type Locale } from "./prompt.ts";
import { buildTools } from "./tools.ts";

/** Longest reply returned (the api rejects longer ones). */
export const MAX_REPLY_CHARS = 8000;

/** Body of `POST /v1/chat`, sent by the Rust api. */
export const chatRequestSchema = z
  .object({
    chat_id: z.string().min(1).max(100),
    tenant: z.string().min(1).max(200),
    locale: z.enum(["es", "en"]),
    grant: z.string().min(1).max(1024),
    token_budget: z.number().int().min(0).max(Number.MAX_SAFE_INTEGER),
    messages: z
      .array(
        z.object({
          role: z.enum(["user", "assistant"]),
          content: z.string().min(1).max(2000),
        }),
      )
      .min(1)
      .max(20),
  })
  .refine((r) => r.messages.at(-1)?.role === "user", { message: "last message must be from the user" });

export type ChatRequest = z.infer<typeof chatRequestSchema>;

export type AgentStop = "done" | "max_turns" | "token_budget";

export interface ChatReply {
  reply: string;
  stop: AgentStop;
  usage: { input_tokens: number; output_tokens: number };
}

/** The run exceeded `AGENT_RUN_TIMEOUT_MS`. */
export class RunTimeout extends Error {
  constructor() {
    super("run timed out");
    this.name = "RunTimeout";
  }
}

/** The provider or the api failed; details are deliberately dropped. */
export class RunFailed extends Error {
  constructor() {
    super("run failed");
    this.name = "RunFailed";
  }
}

class BudgetExhausted extends Error {
  constructor() {
    super("token budget exhausted");
    this.name = "BudgetExhausted";
  }
}

export interface RunDeps {
  /** Transport for the Rust api (tests inject one). */
  apiFetch?: typeof fetch;
  /** Transport for the model provider before the allowlist (tests). */
  providerFetch?: typeof fetch;
}

function textOf(message: Message | undefined): string {
  if (!message || message.role !== "assistant") return "";
  if (typeof message.content === "string") return message.content;
  return message.content
    .flatMap((part) => (part.type === "text" ? [part.text] : []))
    .join("")
    .trim();
}

function lastAssistant(messages: Message[]): Message | undefined {
  for (let i = messages.length - 1; i >= 0; i--) {
    if (messages[i]?.role === "assistant") return messages[i];
  }
  return undefined;
}

/**
 * Runs one chat. Resolves `null` when `outer` aborted (the api or browser
 * went away); throws [`RunTimeout`] or [`RunFailed`].
 */
export async function runChat(
  config: Config,
  request: ChatRequest,
  outer: AbortSignal,
  deps: RunDeps = {},
): Promise<ChatReply | null> {
  const locale: Locale = request.locale;
  const controller = new AbortController();
  let timedOut = false;
  let failed = false;
  let budgetHit = false;
  let maxTurns = false;

  const onOuter = () => controller.abort();
  outer.addEventListener("abort", onOuter, { once: true });
  if (outer.aborted) controller.abort();
  const timer = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, config.limits.runTimeoutMs);

  const api = new ApiClient({
    baseUrl: config.apiUrl,
    serviceToken: config.serviceToken,
    grant: request.grant,
    ...(deps.apiFetch ? { fetch: deps.apiFetch } : {}),
  });

  const used = { input: 0, output: 0 };
  let report: Promise<void> = Promise.resolve();
  const exhausted = () => budgetHit || used.input + used.output >= request.token_budget;

  const history = request.messages.slice(0, -1).map(
    (m): Message => (m.role === "user" ? { role: "user", content: m.content } : { role: "assistant", content: m.content }),
  );
  const prompt = request.messages.at(-1)?.content ?? "";

  const apiKey = config.apiKey();
  const options: AgentOptions = {
    provider: config.provider,
    model: config.model,
    system: systemPrompt(locale),
    priorMessages: history,
    tools: buildTools(api, config.limits.toolTimeoutMs, config.limits.maxToolResultChars),
    maxTurns: config.limits.maxTurns,
    maxTurnExtensions: 0,
    maxTokens: config.limits.maxOutputTokens,
    signal: controller.signal,
    fetch: allowlistedFetch(config.providerHosts, deps.providerFetch),
    webSearch: false,
    compaction: false,
    clearToolUses: false,
    maxToolResultChars: config.limits.maxToolResultChars,
    maxTurnToolResultChars: config.limits.maxToolResultChars * 2,
    // Before every model call: wait for the last usage report and stop when
    // the budget is spent, so overshoot is bounded by one turn.
    transformContext: async (messages) => {
      await report;
      if (exhausted()) {
        budgetHit = true;
        controller.abort();
        // Throwing ends the loop before the model is called again.
        throw new BudgetExhausted();
      }
      return messages;
    },
    ...(apiKey ? { apiKey } : {}),
  };

  try {
    const agent = new Agent(options);
    try {
      const stream = agent.prompt(prompt);
      // Take the iterator first (marks us as the consumer), then handle the
      // stream's result promise: it rejects whenever the loop throws, and
      // the outcome is read from the events below.
      const iterator = stream[Symbol.asyncIterator]();
      stream.then(undefined, () => undefined);
      for await (const event of { [Symbol.asyncIterator]: () => iterator }) {
        switch (event.type) {
          case "turn_end": {
            const input = Math.max(0, Math.trunc(event.usage.inputTokens));
            const output = Math.max(0, Math.trunc(event.usage.outputTokens));
            used.input += input;
            used.output += output;
            report = report.then(async () => {
              try {
                const { remaining_tokens } = await api.usage(
                  input,
                  output,
                  AbortSignal.timeout(config.limits.toolTimeoutMs),
                );
                if (remaining_tokens <= 0) budgetHit = true;
              } catch {
                // Without the authoritative count the budget cannot be
                // enforced: fail closed.
                failed = true;
                controller.abort();
              }
            });
            // The budget is checked before the next model call
            // (`transformContext`), so a final answer is never thrown away.
            break;
          }
          case "max_turns":
            maxTurns = true;
            break;
          case "error":
            if (!controller.signal.aborted) failed = true;
            break;
          default:
            break;
        }
      }
    } catch {
      if (!controller.signal.aborted) failed = true;
    }
    await report;

    if (timedOut) throw new RunTimeout();
    if (outer.aborted) return null;
    if (failed) throw new RunFailed();

    const usage = { input_tokens: used.input, output_tokens: used.output };
    if (controller.signal.aborted) {
      if (budgetHit) {
        return { reply: fixedMessage("token_budget", locale), stop: "token_budget", usage };
      }
      throw new RunFailed();
    }
    if (maxTurns) return { reply: fixedMessage("max_turns", locale), stop: "max_turns", usage };
    const text = textOf(lastAssistant(agent.getMessages())) || fixedMessage("empty", locale);
    return { reply: Array.from(text).slice(0, MAX_REPLY_CHARS).join(""), stop: "done", usage };
  } finally {
    clearTimeout(timer);
    outer.removeEventListener("abort", onOuter);
  }
}
