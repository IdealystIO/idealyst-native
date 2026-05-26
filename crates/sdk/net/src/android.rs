//! Android transport — placeholder for Phase 4.
//!
//! The full implementation will reach into `java.net.HttpURLConnection`
//! via JNI, running the blocking call on a worker thread and bridging
//! completion back to Rust via a `futures-channel::oneshot`. Picked
//! HttpURLConnection over OkHttp so the SDK has zero Gradle/JAR
//! footprint — see `crates/sdk/net/Cargo.toml` header for the rationale.

use std::time::Duration;

use crate::cancel::CancelToken;
use crate::error::Error;
use crate::headers::Headers;
use crate::method::Method;
use crate::response::Response;

pub(crate) struct Transport;

impl Transport {
    pub(crate) fn new() -> Self {
        Self
    }
}

pub(crate) async fn send(
    _transport: &Transport,
    _method: Method,
    _url: String,
    _headers: Headers,
    _body: Vec<u8>,
    _timeout: Option<Duration>,
    _cancel: Option<CancelToken>,
) -> Result<Response, Error> {
    Err(Error::Other(
        "net::android transport not yet implemented (Phase 4)".into(),
    ))
}
