//! Proc-macro backing [`offload`](../offload)'s `#[offload::job]` attribute on
//! **native** targets.
//!
//! On web, `offload` re-exports `wasmworker::webworker_fn`, which generates the
//! worker-side dispatch entry so the function can be invoked by name inside a Web
//! Worker. On native there is no worker — the job runs on a `std::thread` and is
//! called through an ordinary function pointer — so the attribute has nothing to
//! generate: it returns the annotated function unchanged.
//!
//! Keeping the attribute present (rather than asking callers to `#[cfg]` it away)
//! means a job is annotated **once** and the call site is identical on every
//! platform.

use proc_macro::TokenStream;

/// No-op passthrough: emits the annotated item verbatim. See the crate docs for
/// why this exists only on native.
#[proc_macro_attribute]
pub fn job(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
