# 0004. gg-agent sidecar for AI

- Status: Accepted, amended by [0013](0013-assistant-sidecar-interface.md)
- Date: 2026-10-18

## Context

Some features (fuzzy product matching, extracting data from messy pages, Spanish summaries) benefit from LLMs. Embedding model SDKs and prompts in the Rust services would couple core code to fast-changing AI tooling.

## Decision

AI work runs in a separate gg-agent sidecar process. Rust crates call it over a narrow, versioned interface and treat its output as untrusted input that is validated before use. No AI SDK is linked into `domain`, `matching` or the adapters.

## Consequences

- AI tooling can change without touching core crates.
- Another process to deploy and monitor; callers need timeouts and a non-AI fallback.
- The sidecar interface gets its own module doc and ADR when it is built.

## Update (2026-10-20)

The sidecar is built: `services/agent` runs `@kenkaiiii/gg-agent` headless in Node 22 ([agent.md](../modules/agent.md)), and [ADR 0013](0013-assistant-sidecar-interface.md) records its interface. The sidecar never writes data. Its tools call the Rust api, which derives the tenant from a signed per-chat grant. Match tie-breaks are propose-then-confirm: the agent can only create a suggestion, and a user's click records the decision for their own tenant with who decided. The non-AI fallback is the existing search and product pages; the assistant answers 503 when the sidecar is not configured.
