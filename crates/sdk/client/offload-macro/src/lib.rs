//! Proc-macro backing [`offload`](../offload)'s `#[offload::job]` attribute.
//!
//! The attribute is a marker that generates nothing, on every target: a job is
//! dispatched through its function pointer, which `offload::handle!` captures.
//! Natively that pointer is called on a `std::thread`; on web its
//! function-table index is sent to a Web Worker running the same module
//! (`web_glue::worker`), where the pointer names the same function.
//!
//! Keeping the attribute (rather than asking callers to drop it) means a job
//! reads as one at its definition, and existing code keeps compiling.

use proc_macro::TokenStream;

/// No-op passthrough: emits the annotated item verbatim. See the crate docs.
#[proc_macro_attribute]
pub fn job(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
