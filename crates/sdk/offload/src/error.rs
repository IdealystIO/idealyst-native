//! The unified error type returned by [`run`](crate::run).

/// Why an offloaded job failed to produce a result.
#[derive(Debug, thiserror::Error)]
pub enum OffloadError {
    /// The worker (web) or thread (native) dropped the job before sending a
    /// result — e.g. it panicked, or the pool was torn down mid-flight.
    #[error("offloaded job was canceled before returning a result")]
    Canceled,
}
