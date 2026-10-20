//! Background job runner for mex-scrape (ADR 0011): consumes the pgmq
//! `search_jobs` and `refresh_jobs` queues, runs the cron schedules that
//! enqueue product refreshes and category browses, and owns every paid
//! call (Bright Data fetches, SP-API lookups), matching and price-history
//! writes.
//!
//! The library holds the jobs, the scheduler and their adapters so
//! integration tests can drive them; `main.rs` is the composition root.

pub mod access_metrics;
pub mod amazon_source;
pub mod brightdata_resume;
pub mod brightdata_store;
pub mod clock;
pub mod config;
pub mod jobs;
pub mod metrics;
pub mod offer_matching;
pub mod schedule;
pub mod secrets;
pub mod sinks;
