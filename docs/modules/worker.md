# worker

## Purpose

Background job runner: fetching retailer and Amazon prices, matching, price history. Placeholder: logs start-up and exits.

## Public API

- Binary `worker` (`cargo run -p worker`).

## Dependencies

- Workspace: `domain`, `db`, `matching`, `retailers`, `amazon`.
- Also allowed: `brightdata`, for the Amazon.com.mx page fallback when SP-API cannot answer ([ADR 0003](../decisions/0003-amazon-sp-api-primary-brightdata-fallback.md)). Not a Cargo dependency yet.

## Owned tables

None directly; data access goes through `db`.
