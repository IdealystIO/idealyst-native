use thiserror::Error;

/// The single error type returned by every public `net` API.
///
/// Transport-specific failures (a `reqwest::Error`, a fetch
/// `JsValue`, an `NSError`) are mapped into one of these variants
/// at the boundary so callers never see platform-specific types.
#[derive(Debug, Error)]
pub enum Error {
    /// URL failed to parse or contained an unsupported scheme.
    #[error("invalid url: {0}")]
    InvalidUrl(String),

    /// Connection refused, DNS failure, TLS failure, or any other
    /// transport-level error before a response was received.
    #[error("network error: {0}")]
    Network(String),

    /// The request's deadline expired before a response arrived.
    #[error("request timed out")]
    Timeout,

    /// A response was received but its status code indicates failure.
    /// Only produced via [`Response::error_for_status`]; otherwise 4xx
    /// and 5xx are returned as normal `Response` values.
    #[error("http error: status {code}")]
    Status {
        /// HTTP status code (4xx or 5xx).
        code: u16,
        /// Optional response body as a UTF-8 string (lossy decoded).
        body: Option<String>,
    },

    /// Failed to serialize a request body (e.g. JSON encoding error).
    #[error("serialize error: {0}")]
    Serialize(String),

    /// Failed to deserialize a response body.
    #[error("deserialize error: {0}")]
    Deserialize(String),

    /// The platform reports the device is offline (web `navigator.onLine`
    /// false, iOS reachability, Android `ConnectivityManager`). Best-effort;
    /// not every transport surfaces this.
    #[error("device is offline")]
    Offline,

    /// The request was aborted via a [`CancelHandle`](crate::CancelHandle).
    /// Surfaces when a `cancel_on(token)` token's paired handle fires
    /// while the request is in flight (or queued).
    #[error("request was cancelled")]
    Cancelled,

    /// Catch-all for transport-specific errors that don't map to any of
    /// the variants above.
    #[error("{0}")]
    Other(String),
}
