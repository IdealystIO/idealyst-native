//! `docs!` proc macro.
//!
//! Parses one structured documentation page declaration and emits:
//!
//! 1. `pub fn page() -> runtime_core::Element` — the renderable
//!    screen.
//! 2. `pub static PAGE_META: docs::PageMeta` — the structured
//!    metadata, consumable by MCP servers, search indexers, and
//!    other introspection tools.
//!
//! See `examples/docs/docs-content-plan/docs-macro-design.md` for
//! the input grammar and design rationale.

use proc_macro::TokenStream;

mod parse;
mod emit;

use parse::DocPage;

#[proc_macro]
pub fn docs(input: TokenStream) -> TokenStream {
    let page = syn::parse_macro_input!(input as DocPage);
    match emit::emit(page) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}
