//! `#[host_fn]`'s records: an app function a bundle calls by name.
//!
//! In the app a `#[host_fn]` is the real function plus a [`HostFnDef`] the
//! app lists in its allowlist (`stream_host::remote::install_with`). In a
//! bundle it's a stub calling ONE wasm import, under [`HOST_FN_MODULE`],
//! named [`import_name`]`(path, schema)` — so the bundle's import section is
//! its list of host functions, and the app refuses a bundle at load that
//! calls one it doesn't export, or exports with a different signature.
//!
//! The same records as `stream-abi`'s (model A's ABI crate): the vocabulary
//! is published and that crate isn't, so a library's `#[host_fn]` (idea-ui)
//! can't name it.

use std::future::Future;
use std::pin::Pin;

/// The wasm import module every host function lives under.
pub const HOST_FN_MODULE: &str = "idealyst_host_fn";

/// An async host function's body: encoded args in, encoded result out.
pub type AsyncCall = fn(Vec<u8>) -> Pin<Box<dyn Future<Output = Vec<u8>>>>;

#[derive(Clone, Copy)]
pub enum HostFnKind {
    /// Import `(args_ptr, args_len) -> reply_len`.
    Sync(fn(&[u8]) -> Vec<u8>),
    /// Import `(args_ptr, args_len, then_callback)`: returns at once; the
    /// app runs the future and calls `then_callback` with the result.
    Async(AsyncCall),
}

/// What an app exports for one host function (`<fn>::export()`).
#[derive(Clone, Copy)]
pub struct HostFnDef {
    /// `module_path!()::fn_name` — the same on both sides.
    pub path: &'static str,
    /// Fingerprint of the signature (argument and return types, asyncness).
    pub schema: u64,
    pub kind: HostFnKind,
}

/// The import name a bundle's stub links against.
pub fn import_name(path: &str, schema: u64) -> String {
    format!("{path}#{schema:016x}")
}

/// Split an import name back into `(path, schema)`.
pub fn parse_import_name(name: &str) -> Option<(&str, u64)> {
    let (path, hash) = name.rsplit_once('#')?;
    Some((path, u64::from_str_radix(hash, 16).ok()?))
}
