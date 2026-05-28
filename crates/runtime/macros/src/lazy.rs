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
    // Tokens-as-bytes hash. Stable across rebuilds because token text is
    // deterministic; unique per call site (different contents → different
    // hash → different chunk name).
    let token_text = input.to_string();
    let hash = stable_hash(&token_text);

    let body_tokens: proc_macro2::TokenStream = input.into();
    let body_ident = syn::Ident::new(&format!("__idealyst_lazy_body_{hash}"), Span::call_site());
    let split_name = syn::Ident::new(&format!("__idealyst_lazy_{hash}"), Span::call_site());

    // Wrap the body in a `#[wasm_split]`-annotated async fn. On web the
    // build's wasm-split post-process (dioxus reloc splitter, run after
    // wasm-bindgen) hoists this function — and everything only it reaches —
    // into a separate chunk wasm loaded on demand. The split is a post-link
    // rewrite of the SINGLE already-bindgen'd module, so it handles ARBITRARY
    // body code uniformly, bindgen/web-sys/wgpu included (no per-module
    // bindgen, no PIC). On native targets the attribute lowers to a plain
    // inline async fn that resolves synchronously.
    //
    // `IntoElement::into_element` coerces whatever the block returns
    // (`Bound<H>`, `Element`, `LazyBuilder`, …) into a bare `Element` — the
    // wasm-split function signature pins the return type concretely.
    let expanded = quote! {
        {
            // The `#[wasm_split]` attribute expands to code referencing the
            // wasm-split runtime via bare `wasm_split::…` paths. Alias
            // runtime-core's re-export into scope so author crates don't need
            // their own `wasm-split` dependency. The `use` reaches the nested
            // fn defined later in the block. `#[allow(unused_imports)]` covers
            // the native expansion, where the attribute names no `wasm_split`.
            #[allow(unused_imports)]
            use ::runtime_core::__wasm_split as wasm_split;
            #[::runtime_core::__wasm_split::wasm_split(#split_name)]
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
