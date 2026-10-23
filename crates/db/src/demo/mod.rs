//! Demo data for local demos only (ADR 0016; `docs/modules/db/demo-data.md`).
//!
//! The `seed_demo` binary only orchestrates these parts:
//! - [`catalog`]: the embedded `config/demo_catalog.json` (products, store
//!   listings, Amazon products, keyword groups), parsed and validated. The
//!   worker's demo mode serves the same catalog.
//! - [`history`]: a deterministic year of daily prices per listing, with
//!   noise, Hot Sale, Buen Fin, December prices and out-of-stock gaps.
//! - [`account`]: the demo login and its memberships.
//! - [`searches`]: one queued demo-account search per keyword group, through
//!   the API's start-search use case.

pub mod account;
pub mod catalog;
pub mod history;
pub mod searches;
