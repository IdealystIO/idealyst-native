//! Keeps a macro's generated `impl`s out of its module's impl numbering.
//!
//! An `impl` has no name, so its symbol carries a number: its position among
//! the module's impls (`{impl#3}`, under v0 mangling `Ms2_`). Hand-written
//! impls are numbered ahead of macro-expanded ones, so a generated impl moves
//! whenever a component is added ahead of it AND whenever a hand-written impl
//! is added anywhere in the file. A renamed function defeats a hot patch: a
//! dependent's base code still calls the old name, so
//! `build_web::hotpatch_additive` has to refuse the save.
//!
//! [`scoped`] gives each macro's impls a parent of their own, a `const` named
//! after the item that owns them. Impls are numbered per parent and a named
//! item carries no number, so neighbours can no longer shift them. Not
//! `const _`: that const is numbered too, so adding one ahead renames what is
//! inside the next. The fn, props struct, tag alias and handle re-export stay
//! at module level, because `ui!` names them by path. Inherent methods stay
//! inherent wherever the impl sits, so they still win over the
//! `ApplyLiteralFallback` blanket impl.
//!
//! Gated on `hot-reload`, the only build that patches: a production expansion
//! keeps its impls at module level and pays no tokens for this.
//!
//! Under an IDE proc-macro server the const is `_` instead. rust-analyzer only
//! indexes impls in unnamed consts, so inside a named one every method call is
//! a false E0599 in the editor (checked against rust-analyzer 0.3.3097).
//! `#[cfg(rust_analyzer)]` is not usable here: emitted into a consumer crate
//! it trips `unexpected_cfgs`, see `recovery::host_has_no_span_lines`.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use runtime_macros_parse::recovery::host_has_no_span_lines;
use syn::Ident;

/// Who owns a scope. Part of its name, because one type can get a scope from
/// more than one macro (`#[props]` and `#[component]` on the same name).
#[derive(Clone, Copy)]
pub(crate) enum Owner {
    Component,
    Props,
    Sheet,
}

impl Owner {
    fn prefix(self) -> &'static str {
        match self {
            Owner::Component => "component",
            Owner::Props => "props",
            Owner::Sheet => "sheet",
        }
    }
}

/// The const's name under rustc: `__<owner>_scope_<name>`. It never appears
/// in source, so it cannot trip the additive check's freshness rule, which
/// compares new names against identifiers a file mentions.
fn scope_ident(owner: Owner, name: &Ident) -> Ident {
    format_ident!("__{}_scope_{}", owner.prefix(), name)
}

/// `items`, wrapped per the module docs when this build patches.
pub(crate) fn scoped(owner: Owner, name: &Ident, items: TokenStream2) -> TokenStream2 {
    if !cfg!(feature = "hot-reload") {
        return items;
    }
    wrap(owner, name, items)
}

fn wrap(owner: Owner, name: &Ident, items: TokenStream2) -> TokenStream2 {
    if items.is_empty() {
        return items;
    }
    let ident = if host_has_no_span_lines(&quote!(#name)) {
        quote!(_)
    } else {
        let ident = scope_ident(owner, name);
        quote!(#ident)
    };
    quote! {
        #[allow(non_upper_case_globals, non_local_definitions)]
        const #ident: () = { #items };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;

    #[test]
    fn scope_names_differ_per_owner_and_keep_raw_names_valid() {
        let foo = Ident::new("Foo", Span::call_site());
        assert_eq!(scope_ident(Owner::Component, &foo).to_string(), "__component_scope_Foo");
        assert_eq!(scope_ident(Owner::Props, &foo).to_string(), "__props_scope_Foo");
        let raw = Ident::new_raw("type", Span::call_site());
        assert_eq!(scope_ident(Owner::Sheet, &raw).to_string(), "__sheet_scope_type");
    }

    /// An empty expansion must stay empty: a `const` with nothing in it would
    /// be tokens for no reason in every feature-off path of the callers.
    #[test]
    fn nothing_to_scope_emits_nothing() {
        let foo = Ident::new("Foo", Span::call_site());
        assert!(wrap(Owner::Props, &foo, TokenStream2::new()).is_empty());
    }

    /// Unit-test spans are degenerate like an IDE server's, so this is the
    /// IDE path: the unnamed const rust-analyzer can see into.
    #[test]
    fn an_ide_host_gets_the_unnamed_const() {
        let foo = Ident::new("Foo", Span::call_site());
        let out = wrap(Owner::Props, &foo, quote!(impl Foo {})).to_string();
        assert!(out.contains("const _ : ()"), "{out}");
        assert!(out.contains("non_local_definitions"), "{out}");
    }
}
