# domain

## Purpose

Core domain types and pure business logic (products, prices, stores). No I/O; every other crate may use it.

## Public API

- `PRODUCT_NAME`: product name constant. Domain types are added as features land.

## Dependencies

- Workspace: none (enforced by `scripts/check_docs.py`).
- External: none.

## Owned tables

None. Persistence lives in `db`.
