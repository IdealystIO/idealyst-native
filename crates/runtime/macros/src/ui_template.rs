//! The TEMPLATE lowering of a `ui!` body — phase-2 half.
//!
//! See `ui_split` for the shared front half and
//! `crates/runtime/template/README.md` for the descriptor model.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::ui::UiNode;

/// Placeholder until the template emitter lands (phase 2). Emitting a
/// loud `compile_error!` rather than silently falling back to the direct
/// lowering: a silent fallback would make the parity suite green while
/// proving nothing.
pub(crate) fn emit(_nodes: &[UiNode], _input: &TokenStream2) -> TokenStream2 {
    quote! {
        ::std::compile_error!(
            "`ui_lowered!(template)` is not implemented yet (phase 2)."
        )
    }
}
