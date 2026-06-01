use std::convert::Infallible;

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The unified error type for server functions.
///
/// Generic over the author's domain error `E`, which defaults to `String` for
/// the stringly-typed ergonomics carried over from v0 (and to keep `Result<T,
/// ServerError>` a valid bare spelling). The domain error rides in
/// [`Self::Failed`] and is serialized across the wire — both sides share its
/// definition through the shared `api` crate. Every other variant is a
/// transport- or protocol-level failure that carries no domain payload and is
/// therefore constructible for *any* `E`; see [`TransportError`].
///
/// Authors upgrade from stringly errors to typed ones simply by spelling the
/// return type `Result<T, ServerError<MyError>>` — the macro and dispatch are
/// already generic over the whole return type, so nothing else changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Error)]
pub enum ServerError<E = String> {
    /// The server-side function body returned `Err(e)`. `e` is the author's
    /// typed domain error, serialized across the wire.
    #[error("server fn failed: {0}")]
    Failed(E),

    /// Transport-layer failure (DNS, TCP, TLS, timeout). Only ever observed on
    /// the client side; the server surfaces failures through [`Self::Failed`] /
    /// [`Self::Server`].
    #[error("network error: {0}")]
    Network(String),

    /// Could not serialize the args or deserialize the response. A *same-version*
    /// codec bug — distinct from [`Self::IncompatibleVersion`], which is a codec
    /// failure attributable to client/server schema drift.
    #[error("codec error: {0}")]
    Codec(String),

    /// Server returned an unexpected status (not produced by a `Failed` return —
    /// i.e. the dispatcher itself rejected the request: unknown path, malformed
    /// body, internal panic).
    #[error("server error ({status}): {message}")]
    Server { status: u16, message: String },

    /// The call was aborted via a `net::CancelToken` before the response
    /// arrived. Mirrors `net::Error::Cancelled` at the server-fn level.
    #[error("server fn call was cancelled")]
    Cancelled,

    /// The client and server schemas for this function are no longer
    /// interoperable. Surfaced when a payload fails to (de)serialize *and* the
    /// schema hashes differ (DESIGN.md §5), or when a `strict_version` endpoint
    /// rejects a hash mismatch up front. This is the actionable "your app is
    /// outdated" signal, deliberately distinct from a same-version
    /// [`Self::Codec`] bug. (The negotiation that produces it lands in a later
    /// phase; the variant is defined now so the wire enum is stable.)
    #[error("incompatible version for '{path}' (client schema {client_schema:#x}, server schema {server_schema:#x})")]
    IncompatibleVersion {
        path: String,
        client_schema: u64,
        server_schema: u64,
    },
}

impl ServerError<String> {
    /// Convenience constructor for a stringly-typed domain failure. Use inside a
    /// server-fn body: `return Err(ServerError::failed("not found"))`.
    pub fn failed(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

impl ServerError<Infallible> {
    /// Lift a transport-level error into a domain-carrying `ServerError<E>`.
    ///
    /// The queue / HTTP / codec layers can only ever produce the non-`Failed`
    /// variants, so they work in `ServerError<Infallible>` ([`TransportError`]).
    /// At the boundary where a concrete return type is known — the client's
    /// `call_impl`, the server's batch re-encode — that transport error is
    /// re-tagged to the caller's `E`. The `Failed` arm is uninhabited
    /// (`Infallible`), so the match is total and allocation-free.
    pub fn into_domain<E>(self) -> ServerError<E> {
        match self {
            ServerError::Failed(never) => match never {},
            ServerError::Network(s) => ServerError::Network(s),
            ServerError::Codec(s) => ServerError::Codec(s),
            ServerError::Server { status, message } => ServerError::Server { status, message },
            ServerError::Cancelled => ServerError::Cancelled,
            ServerError::IncompatibleVersion {
                path,
                client_schema,
                server_schema,
            } => ServerError::IncompatibleVersion {
                path,
                client_schema,
                server_schema,
            },
        }
    }
}

/// A [`ServerError`] that carries no domain payload.
///
/// The client-side batch queue / HTTP path and the server-side codec layer can
/// only fail in non-`Failed` ways, so they operate in this monomorphization and
/// fold into a `ServerError<E>` via [`ServerError::into_domain`] once a concrete
/// return type is in hand.
pub type TransportError = ServerError<Infallible>;

/// Implemented by every type that can be the return type of a `#[server]`
/// function.
///
/// The trait provides a fallible construction path for the client stub: when the
/// network call fails or the response can't be decoded, the stub builds a return
/// value out of a [`ServerError`] via this trait instead of panicking or
/// returning a magic value. The associated [`Self::Error`] is the domain error
/// `E` carried by this return type's `ServerError<E>`, so transport failures fold
/// into the *same* error type the body returns.
pub trait ServerFnReturn: Sized {
    /// The domain error `E` of this return type's `ServerError<E>`.
    type Error;

    /// Produce a return value representing a server-fn-level failure (transport,
    /// codec, version, or non-2xx). Distinct from the user's own domain error
    /// variants — those arrive via the deserialised payload, not through here.
    fn from_server_error(error: ServerError<Self::Error>) -> Self;
}

impl<T, E> ServerFnReturn for Result<T, ServerError<E>> {
    type Error = E;
    fn from_server_error(error: ServerError<E>) -> Self {
        Err(error)
    }
}
