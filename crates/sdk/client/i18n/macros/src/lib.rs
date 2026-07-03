//! Proc-macro half of the `i18n` SDK — the `i18n! { … }` translation macro.
//!
//! Parses inline, Rust-native translations and emits a strongly-typed
//! `Locale` enum plus one function per message. Validation errors are
//! collected (not just the first) and each is spanned at the offending
//! item, mirroring `runtime-macros`' `doc_check` diagnostics.
//!
//! Depend on the `i18n` crate, which re-exports this macro as `i18n::i18n`.

mod emit;
mod parse;
mod validate;

use proc_macro::TokenStream;
use syn::parse_macro_input;

/// Define a localized message catalog. See the `i18n` crate docs for the
/// DSL and the compile-time guarantees.
#[proc_macro]
pub fn i18n(input: TokenStream) -> TokenStream {
    let parsed = parse_macro_input!(input as parse::I18nInput);
    match validate::check(&parsed) {
        Ok(model) => emit::emit(&parsed, &model).into(),
        Err(errors) => {
            let mut ts = proc_macro2::TokenStream::new();
            for e in errors {
                ts.extend(e.to_compile_error());
            }
            ts.into()
        }
    }
}
