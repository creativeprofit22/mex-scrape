//! One clock for both clock ports (`brightdata::runs::Clock` and
//! `app::Clock`), so jobs stamp runs, rows and caps from the same source.
//! Tests inject their own.

use brightdata::runs::{Clock as RunClock, SystemClock};
use domain::UnixMillis;

/// A clock usable by the Bright Data runner and by the `app` ports.
pub trait JobClock: RunClock + app::Clock {}

impl<T: RunClock + app::Clock> JobClock for T {}

/// The system wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct WallClock;

impl RunClock for WallClock {
    fn now_ms(&self) -> i64 {
        SystemClock.now_ms()
    }
}

impl app::Clock for WallClock {
    fn now(&self) -> UnixMillis {
        UnixMillis(SystemClock.now_ms())
    }
}
