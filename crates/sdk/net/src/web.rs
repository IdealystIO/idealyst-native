//! Web (wasm) transport — placeholder for Phase 2.
//!
//! The full implementation will use `gloo-net::http::Request` and
//! `wasm-bindgen-futures::JsFuture`. For now this module is just enough
//! shape to let the crate compile on the wasm target; calling
//! [`send`] returns `Error::Other("not implemented")`.

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
        "net::web transport not yet implemented (Phase 2)".into(),
    ))
}
