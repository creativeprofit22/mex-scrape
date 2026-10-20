/**
 * HTTP surface: `POST /v1/chat` (service bearer required) and `GET /health`.
 * Logs are JSON lines with fixed messages; request bodies, replies, tokens
 * and provider errors are never logged.
 */
import { createHash, timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";

import type { Config } from "./config.ts";
import { chatRequestSchema, runChat, RunTimeout, type RunDeps } from "./run.ts";

/** Largest accepted request body. */
export const MAX_BODY_BYTES = 64 * 1024;

export type LogFn = (msg: string, fields?: Record<string, string | number | boolean>) => void;

export const jsonLog: LogFn = (msg, fields = {}) => {
  process.stdout.write(`${JSON.stringify({ time: new Date().toISOString(), msg, ...fields })}\n`);
};

function digest(value: string): Buffer {
  return createHash("sha256").update(value, "utf8").digest();
}

function authorized(req: IncomingMessage, expected: Buffer): boolean {
  const header = req.headers.authorization;
  if (typeof header !== "string" || !header.startsWith("Bearer ")) return false;
  return timingSafeEqual(digest(header.slice("Bearer ".length)), expected);
}

function send(res: ServerResponse, status: number, body: unknown): void {
  if (res.headersSent || res.destroyed) return;
  const text = JSON.stringify(body);
  res.writeHead(status, {
    "content-type": "application/json",
    "content-length": Buffer.byteLength(text),
    "cache-control": "no-store",
  });
  res.end(text);
}

class BodyTooLarge extends Error {}

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    let size = 0;
    req.on("data", (chunk: Buffer) => {
      size += chunk.length;
      if (size > MAX_BODY_BYTES) {
        req.removeAllListeners("data");
        reject(new BodyTooLarge());
        req.resume();
        return;
      }
      chunks.push(chunk);
    });
    req.on("end", () => resolve(Buffer.concat(chunks).toString("utf8")));
    req.on("error", reject);
  });
}

/**
 * Backstop: if a library ever leaves a rejection unhandled (e.g. a provider
 * error after the run already failed), Node would print it and crash; log a fixed
 * message instead and keep serving.
 */
export function installProcessGuards(log: LogFn = jsonLog): void {
  process.on("unhandledRejection", () => log("unhandled_rejection"));
}

export interface ServerDeps extends RunDeps {
  log?: LogFn;
}

/** Builds (but does not start) the sidecar server. */
export function createAgentServer(config: Config, deps: ServerDeps = {}): Server {
  const log = deps.log ?? jsonLog;
  const expected = digest(config.serviceToken);
  const busy = new Set<string>();

  return createServer((req, res) => {
    void handle(req, res).catch(() => {
      log("request_failed");
      send(res, 500, { error: "internal" });
    });
  });

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const path = (req.url ?? "").split("?")[0];
    if (req.method === "GET" && path === "/health") {
      send(res, 200, { status: "ok" });
      return;
    }
    if (path !== "/v1/chat") {
      send(res, 404, { error: "not_found" });
      return;
    }
    if (req.method !== "POST") {
      send(res, 405, { error: "method_not_allowed" });
      return;
    }
    if (!authorized(req, expected)) {
      log("chat_unauthorized");
      send(res, 401, { error: "unauthenticated" });
      return;
    }

    let raw: string;
    try {
      raw = await readBody(req);
    } catch (e) {
      send(res, e instanceof BodyTooLarge ? 413 : 400, { error: "invalid_request" });
      return;
    }
    let json: unknown;
    try {
      json = JSON.parse(raw);
    } catch {
      send(res, 400, { error: "invalid_request" });
      return;
    }
    const parsed = chatRequestSchema.safeParse(json);
    if (!parsed.success) {
      send(res, 422, { error: "invalid_request" });
      return;
    }
    const request = parsed.data;

    if (busy.has(request.tenant)) {
      log("chat_busy");
      send(res, 409, { error: "agent_busy" });
      return;
    }
    busy.add(request.tenant);

    // The api (and through it the browser) going away aborts the run.
    const disconnect = new AbortController();
    const onClose = () => {
      if (!res.writableEnded) disconnect.abort();
    };
    res.on("close", onClose);

    const started = Date.now();
    try {
      const reply = await runChat(config, request, disconnect.signal, deps);
      if (reply === null) {
        log("chat_aborted", { ms: Date.now() - started });
        return;
      }
      log("chat_done", {
        stop: reply.stop,
        ms: Date.now() - started,
        input_tokens: reply.usage.input_tokens,
        output_tokens: reply.usage.output_tokens,
      });
      send(res, 200, reply);
    } catch (e) {
      if (e instanceof RunTimeout) {
        log("chat_timeout", { ms: Date.now() - started });
        send(res, 504, { error: "agent_timeout" });
      } else {
        log("chat_failed", { ms: Date.now() - started });
        send(res, 502, { error: "agent_failed" });
      }
    } finally {
      busy.delete(request.tenant);
      res.off("close", onClose);
    }
  }
}
