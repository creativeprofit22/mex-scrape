//! The calls a run needs, behind a trait so tests can inject fakes (as
//! linkgo `runs.rs`).

use std::future::Future;

use crate::client::{DatasetTrigger, Download, SnapshotStatus, UnlockRequest, UnlockedPage};
use crate::error::BrightDataError;
use crate::ids::SnapshotId;

/// Bright Data operations used by [`crate::runs`]. Implemented by
/// [`crate::BrightDataClient`]; tests implement it with fakes. Implementations
/// must return redacted, URL-free errors.
pub trait Transport: Send + Sync {
    /// Web Unlocker: fetches one page.
    fn unlock(
        &self,
        request: &UnlockRequest,
    ) -> impl Future<Output = Result<UnlockedPage, BrightDataError>> + Send;

    /// Datasets v3: starts a collection; returns its snapshot id.
    fn trigger(
        &self,
        trigger: &DatasetTrigger,
    ) -> impl Future<Output = Result<SnapshotId, BrightDataError>> + Send;

    /// Datasets v3: collection progress.
    fn progress(
        &self,
        id: &SnapshotId,
    ) -> impl Future<Output = Result<SnapshotStatus, BrightDataError>> + Send;

    /// Datasets v3: downloads the snapshot (`NotReady` on 202/409).
    fn download(
        &self,
        id: &SnapshotId,
    ) -> impl Future<Output = Result<Download, BrightDataError>> + Send;

    /// Datasets v3: cancels a collection.
    fn cancel(&self, id: &SnapshotId) -> impl Future<Output = Result<(), BrightDataError>> + Send;
}
