use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The unified error type for server functions in v0.
///
/// Carries every failure mode a server-fn call can encounter:
/// - The function body itself returning an `Err` ([`Self::Failed`]).
/// - Transport-layer failures observed only on the client
///   ([`Self::Network`]).
/// - Codec failures (JSON encoding/decoding) on either side
///   ([`Self::Codec`]).
/// - The server returning a non-2xx status not produced by a normal
///   `Failed` return ([`Self::Server`]).
///
/// Authors who want richer domain errors today should encode them
/// inside [`Self::Failed`]'s string (e.g. as JSON) and decode on the
/// client; v1 will parameterise the wrapper with a user error type
/// for type-safe domain errors, à la Leptos's `ServerFnError<E>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum ServerError {
    /// The server-side function body returned `Err(...)`.
    #[error("server fn failed: {0}")]
    Failed(String),

    /// Transport-layer failure (DNS, TCP, TLS, timeout). Only ever
    /// observed on the client side; the server side surfaces failures
    /// through [`Self::Failed`] / [`Self::Server`].
    #[error("network error: {0}")]
    Network(String),

    /// Could not serialize the args or deserialize the response.
    /// Indicates a schema mismatch between client and server builds.
    #[error("codec error: {0}")]
    Codec(String),

    /// Server returned an unexpected status (not produced by a
    /// `Failed` return — i.e. the dispatcher itself rejected the
    /// request: unknown path, malformed body, internal panic).
    #[error("server error ({status}): {message}")]
    Server { status: u16, message: String },

    /// The call was aborted via a `net::CancelToken` before the
    /// response arrived. Mirrors `net::Error::Cancelled` at the
    /// server-fn level.
    #[error("server fn call was cancelled")]
    Cancelled,
}

impl ServerError {
    /// Convenience constructor for `Failed(message)`. Use inside a
    /// server-fn body: `return Err(ServerError::failed("not found"))`.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// Implemented by every type that can be the return type of a
/// `#[server]` function.
///
/// The trait provides a fallible construction path for the client
/// stub: when the network call fails or the response can't be
/// decoded, the stub builds a return value out of a [`ServerError`]
/// via this trait instead of panicking or returning a magic value.
///
/// A blanket impl is provided for `Result<T, ServerError>`, which is
/// the recommended return shape for v0. Custom error types can
/// implement this trait themselves to receive transport failures in
/// their own error variant.
pub trait ServerFnReturn: Sized {
    /// Produce a return value representing a server-fn-level failure
    /// (transport, codec, or non-2xx). Distinct from the user's own
    /// domain error variants — those arrive via the deserialised
    /// payload, not through this trait.
    fn from_server_error(error: ServerError) -> Self;
}

impl<T> ServerFnReturn for Result<T, ServerError> {
    fn from_server_error(error: ServerError) -> Self {
        Err(error)
    }
}
