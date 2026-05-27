//! `lazy! { … }` — inline code-splitting boundary.
//!
//! Wraps a `ui!`-style block in a `#[wasm_split]` async function so
//! the build's wasm-split post-process pulls the body into a separate
//! chunk. The macro emits a `Primitive::Lazy` whose loader awaits
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
//!         #[::wasm_splitter::wasm_split(__idealyst_lazy_<hash>)]
//!         async fn __idealyst_lazy_body_<hash>(
//!             _: (),
//!         ) -> ::runtime_core::Primitive {
//!             ::runtime_core::ui! { Text { "loaded on demand" } }
//!                 .into_primitive()
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
//! - **Return type is `Primitive`.** The block is interpreted as a
//!   `ui!` block — its value is coerced through `IntoPrimitive`.
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
    let token_text = input.to_string();
    let hash = stable_hash(&token_text);

    // Pass the block tokens through as-is. The macro treats the
    // body as a Rust block whose tail expression implements
    // `IntoPrimitive` — author code writes `ui! { … }` (or builds
    // a primitive any other way) inside. This composes with `let`
    // bindings, `switch`, helper calls, etc. without the macro
    // needing to know about the framework's DSL.
    let body_tokens: proc_macro2::TokenStream = input.into();

    let body_ident = syn::Ident::new(
        &format!("__idealyst_lazy_body_{hash}"),
        Span::call_site(),
    );
    let split_name = syn::Ident::new(
        &format!("__idealyst_lazy_{hash}"),
        Span::call_site(),
    );

    // `IntoPrimitive::into_primitive` coerces whatever the block
    // returns (`Bound<H>`, `Primitive`, `LazyBuilder`, …) into a
    // bare `Primitive` — required because the wasm-split function
    // signature pins the return type concretely.
    let expanded = quote! {
        {
            #[::runtime_core::__wasm_split::wasm_split(#split_name)]
            async fn #body_ident(_: ()) -> ::runtime_core::Primitive {
                use ::runtime_core::IntoPrimitive as _;
                { #body_tokens }.into_primitive()
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
