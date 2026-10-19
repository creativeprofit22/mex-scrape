# 0005. Docs stay modular and current

- Status: Accepted
- Date: 2026-10-18

## Context

The codebase is split into many crates plus a web app. Without enforced docs, module boundaries and decisions drift and are lost.

## Decision

- `docs/ARCHITECTURE.md` holds the module map and the allowed-dependencies table.
- Every workspace crate has `docs/modules/<crate>.md` with Purpose, Public API, Dependencies and Owned tables.
- Significant decisions are ADRs in `docs/decisions/NNNN-title.md`; changed decisions get a new ADR that supersedes the old one.
- `scripts/check_docs.py` runs in CI and fails when a crate lacks a module doc, is missing from ARCHITECTURE.md, or depends on a workspace crate its allowed-dependencies row does not list.
- Docs change in the same change as the code.

## Consequences

- Adding a crate needs three doc edits; CI blocks it otherwise.
- Prose accuracy (beyond structure and dependencies) still relies on review.
