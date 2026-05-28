//! `lazy! { … }` — inline code-splitting boundary.
//!
//! Wraps a `ui!`-style block in a `#[wasm_split]` async function so
//! the build's wasm-split post-process pulls the body into a separate
//! chunk. The macro emits a `Element::Lazy` whose loader awaits
//! that async function.
//!
//! # Author API
//!
//! ```ignore
//! use runtime_core::{lazy, ui};
//!
//! ui! {
//!     Text { "always loaded" }
//!     { lazy! { Text { "loaded on demand" } } }
//! }
//! ```
//!
//! Becomes (roughly):
//!
//! ```ignore
//! ui! {
//!     Text { "always loaded" }
//!     { {
//!         // Alias runtime-core's re-export so the attribute's
//!         // `wasm_split::…` expansion resolves without the author
//!         // crate depending on `wasm-split` directly.
//!         use ::runtime_core::__wasm_split as wasm_split;
//!         #[::runtime_core::__wasm_split::wasm_split(__idealyst_lazy_<hash>)]
//!         async fn __idealyst_lazy_body_<hash>(
//!             _: (),
//!         ) -> ::runtime_core::Element {
//!             ::runtime_core::ui! { Text { "loaded on demand" } }
//!                 .into_element()
//!         }
//!         ::runtime_core::primitives::lazy::lazy_split(|| {
//!             ::std::boxed::Box::pin(__idealyst_lazy_body_<hash>(()))
//!         })
//!     } }
//! }
//! ```
//!
//! # v1 constraints
//!
//! - **No captures.** The lazy block can't reference enclosing
//!   variables. (wasm-split's annotated function is a plain `fn`,
//!   not `Fn`; it can't carry state. Capture hoisting via typed
//!   `Args` is the v2 plan.)
//! - **Return type is `Element`.** The block is interpreted as a
//!   `ui!` block — its value is coerced through `IntoElement`.
//!
//! # Naming
//!
//! The function and split-module names are derived from a SHA-256
//! hash of the block's tokens + the call site's `Span` (when
//! available). That keeps names stable across rebuilds but unique
//! per call site, so two identical-shaped `lazy!` blocks in
//! different places get distinct chunks.

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use sha2::{Digest, Sha256};

pub fn emit(input: TokenStream) -> TokenStream {
    // Tokens-as-bytes hash. Stable across rebuilds because token
    // text is deterministic; unique per call site (different
    // contents → different hash → different chunk).
    //
    // Strip ALL whitespace before hashing: build-web's `syn` harvest
    // (which generates the side-module crate in `--dynamic-split` mode)
    // must compute the SAME hash from the same body, but it runs in a
    // non-proc-macro context where `proc_macro2`'s fallback `to_string`
    // spaces tokens differently than `proc_macro`'s here. Token *content*
    // is identical across the two; only whitespace differs. Stripping it
    // makes the hash agree on both sides. (It can mangle string-literal
    // internals, but identically on both sides — the hash only needs to
    // be consistent, not reversible.)
    let normalized: String = input.to_string().chars().filter(|c| !c.is_whitespace()).collect();
    let hash = stable_hash(&normalized);

    // `--dynamic-split` web build: the body is NOT compiled into the main
    // module — build-web harvests it into a generated PIC `--shared` side
    // module (`__idealyst_lazy_body_<hash>`). Here we emit ONLY a stub that
    // fetches + dynamically links that module on demand, keyed by the body
    // hash. Dropping `input` is the whole point: the body must not land in
    // main. The flag is set by build-web's dynamic-split cargo invocation;
    // every other build (the default `#[wasm_split]` path below, all native
    // targets) is unaffected.
    if std::env::var("IDEALYST_DYNAMIC_SPLIT").is_ok() {
        let hash_lit = proc_macro2::Literal::string(&hash);
        return quote! {
            ::runtime_core::primitives::lazy::lazy_split(|| {
                ::runtime_core::primitives::lazy::__dynlink_load(#hash_lit)
            })
        }
        .into();
    }

    // INLINE (default): the body compiles into the SAME binary as a plain
    // async fn and `lazy_split` awaits it — it resolves on first poll, so the
    // subtree mounts after one async tick (brief placeholder, no fetch). This
    // is the behavior on native AND on web builds that don't split (the dev
    // loop, `idealyst build --web --no-split`, or a project with no `lazy!`).
    //
    // The dioxus `#[wasm_split]` reloc path is retired — wasm dynamic linking
    // (the `IDEALYST_DYNAMIC_SPLIT` branch above) is the sole web splitter, so
    // this macro no longer references the `wasm-split` runtime at all.
    let body_tokens: proc_macro2::TokenStream = input.into();
    let body_ident = syn::Ident::new(&format!("__idealyst_lazy_body_{hash}"), Span::call_site());

    // `IntoElement::into_element` coerces whatever the block returns
    // (`Bound<H>`, `Element`, `LazyBuilder`, …) into a bare `Element` — the
    // async fn's return type pins it concretely.
    let expanded = quote! {
        {
            async fn #body_ident(_: ()) -> ::runtime_core::Element {
                use ::runtime_core::IntoElement as _;
                { #body_tokens }.into_element()
            }
            ::runtime_core::primitives::lazy::lazy_split(|| {
                ::std::boxed::Box::pin(#body_ident(()))
            })
        }
    };

    expanded.into()
}

fn stable_hash(input: &str) -> String {
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    // 12 hex chars = 48 bits of name uniqueness. Way more than
    // enough for in-crate collision resistance.
    let bytes = h.finalize();
    bytes[..6]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>()
}
