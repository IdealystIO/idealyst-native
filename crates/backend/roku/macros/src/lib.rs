//! Proc macros for the Roku backend. Currently exposes a single
//! attribute, `#[method]`, that transpiles a restricted subset of
//! Rust into BrightScript.
//!
//! # `#[method]`
//!
//! ```ignore
//! use backend_roku_macros::method;
//!
//! #[method]
//! pub fn factorial(n: i32) -> i32 {
//!     if n <= 1 { 1 } else { n * factorial(n - 1) }
//! }
//! ```
//!
//! Expansion:
//!
//! ```ignore
//! pub fn factorial(n: i32) -> i32 {
//!     if n <= 1 { 1 } else { n * factorial(n - 1) }
//! }
//!
//! /// BrightScript translation of `factorial` emitted by `#[method]`.
//! pub const FACTORIAL_BRS: &str = "\
//! function factorial(n as integer) as integer
//!     if n <= 1 then
//!         return 1
//!     else
//!         return n * factorial(n - 1)
//!     end if
//! end function
//! ";
//! ```
//!
//! At build time, a downstream tool collects every `<NAME>_BRS`
//! constant exported from the user's crates and concatenates them
//! into a single `.brs` source file shipped inside the Roku .pkg.
//! The Rust definition still works as a normal Rust function on
//! iOS / Android / Web — `#[method]` is a pure-additive pass-through.
//!
//! # Pass-through composability
//!
//! `#[method]` re-emits the annotated `fn` byte-for-byte. That means
//! you can stack other attribute macros on the same function and
//! their semantics are preserved:
//!
//! ```ignore
//! #[some_other_attr]
//! #[method]
//! fn helper() { ... }
//! ```
//!
//! Attribute macros expand inner-most-first, so `#[method]` runs
//! first, emits its BRS const + the unchanged `fn`, and the next
//! attribute then sees the original function as if `#[method]`
//! weren't there. `runtime-macros` therefore never has to know
//! about Roku.
//!
//! # What's transpilable
//!
//! See [`backend_roku_transpile`]'s docs for the exact supported
//! subset. Anything outside it becomes a compile-time error at the
//! offending span, not a runtime surprise.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, ItemFn};

#[proc_macro_attribute]
pub fn method(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let item_fn = parse_macro_input!(item as ItemFn);
    match expand(&item_fn) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(item: &ItemFn) -> syn::Result<TokenStream2> {
    let brs = backend_roku_transpile::transpile_fn(item)?;
    let fn_name = item.sig.ident.to_string();
    let const_name = syn::Ident::new(
        &format!("{}_BRS", fn_name.to_uppercase()),
        item.sig.ident.span(),
    );
    let docs = format!(
        "BrightScript translation of `{}` emitted by `#[method]`. \
         Collect at build time into a .brs file.",
        fn_name
    );
    let vis = &item.vis;
    Ok(quote! {
        #item

        #[doc = #docs]
        #[allow(dead_code)]
        #vis const #const_name: &str = #brs;
    })
}
