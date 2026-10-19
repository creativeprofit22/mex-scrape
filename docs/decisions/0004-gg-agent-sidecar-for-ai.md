# 0004. gg-agent sidecar for AI

- Status: Accepted
- Date: 2026-10-18

## Context

Some features (fuzzy product matching, extracting data from messy pages, Spanish summaries) benefit from LLMs. Embedding model SDKs and prompts in the Rust services would couple core code to fast-changing AI tooling.

## Decision

AI work runs in a separate gg-agent sidecar process. Rust crates call it over a narrow, versioned interface and treat its output as untrusted input that is validated before use. No AI SDK is linked into `domain`, `matching` or the adapters.

## Consequences

- AI tooling can change without touching core crates.
- Another process to deploy and monitor; callers need timeouts and a non-AI fallback.
- The sidecar interface gets its own module doc and ADR when it is built.
