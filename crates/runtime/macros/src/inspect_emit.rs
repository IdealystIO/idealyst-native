//! Component-registry emission: what lets the inspector list every
//! mounted `#[component]` with its props.
//!
//! `#[component]` wraps an `Element`-returning body as
//!
//! ```text
//! let __idealyst_inspect = ::runtime_core::__inspect::__inspect_component(
//!     "Name", file!(), line!(), || vec![ /* one probe per prop */ ]);
//! __idealyst_inspect.finish(::runtime_core::component_scope(move || { … }))
//! ```
//!
//! and `#[props]` implements `InspectProps` for its struct, which the
//! explicit-props form (`props: &FooProps`) reads through. The runtime side
//! (`runtime_vocabulary::robot_methods` / `robot_props`) documents why the
//! probes cost nothing outside robot builds and how they classify a prop.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{FnArg, ItemFn, Pat, Type};

/// A type as a one-line source string: `Vec<String>`, `&'a str`,
/// `Rc<dyn Fn(i32)>`. `quote!` separates every token with a space; this
/// drops the spaces that hug punctuation and keeps the ones between two
/// word tokens (`dyn Fn`). It must never split an ident — an earlier
/// version turned `i32` into `i 3 2`.
pub(crate) fn render_type(ty: &Type) -> String {
    const PUNCT: &[char] = &['<', '>', ',', '(', ')', '&', '\'', ':', ';', '[', ']'];
    let raw = quote!(#ty).to_string();
    let chars: Vec<char> = raw.chars().collect();
    let mut out = String::with_capacity(raw.len());
    for (i, &ch) in chars.iter().enumerate() {
        if ch == ' ' {
            let prev = out.chars().last();
            let next = chars.get(i + 1).copied();
            let hugs_punct = prev.map_or(true, |p| PUNCT.contains(&p))
                || next.map_or(true, |n| PUNCT.contains(&n));
            if hugs_punct {
                // `, ` reads better than `,` in a signature.
                if prev == Some(',') {
                    out.push(' ');
                }
                continue;
            }
        }
        out.push(ch);
    }
    // The macros spell framework types by absolute path
    // (`::runtime_core::Reactive<…>`); a reader wants the name the author
    // wrote.
    for prefix in ["::runtime_core::", "::runtime_vocabulary::glue::"] {
        out = out.replace(prefix, "");
    }
    out
}

/// One probe expression for a named value of type `ty`.
fn probe_entry(name: &str, ty: &Type, value: TokenStream2) -> TokenStream2 {
    let ty_str = render_type(ty);
    quote! {
        ::runtime_core::__inspect::entry(
            #name,
            #ty_str,
            (&&&&&::runtime_core::__inspect::PropProbe(#value)).__probe(),
        )
    }
}

/// The `Vec<PropEntry>` expression for a component fn's props.
///
/// - Explicit-props form (one `props` param): read through the struct's
///   `InspectProps` impl (from `#[props]`), empty for any other struct.
/// - Inline form: one probe per ident parameter, skipping the injected
///   `bind_to` handle prop (it is plumbing, not author data).
/// - Generic fns: empty. Autoref specialization only resolves at a
///   concrete call site (`robot_props` module docs).
fn props_expr(item_fn: &ItemFn, legacy_props: bool, bind_to_injected: bool) -> TokenStream2 {
    if !item_fn.sig.generics.params.is_empty() {
        return quote! { ::std::vec::Vec::new() };
    }
    if legacy_props {
        if let Some(FnArg::Typed(pt)) = item_fn.sig.inputs.first() {
            if let Pat::Ident(pi) = &*pt.pat {
                let ident = &pi.ident;
                return quote! {
                    (&&::runtime_core::__inspect::PropsProbe(&#ident)).__props()
                };
            }
        }
        return quote! { ::std::vec::Vec::new() };
    }
    let entries = item_fn.sig.inputs.iter().filter_map(|arg| {
        let FnArg::Typed(pt) = arg else { return None };
        let Pat::Ident(pi) = &*pt.pat else { return None };
        if bind_to_injected && pi.ident == "bind_to" {
            return None;
        }
        let ident = &pi.ident;
        Some(probe_entry(&ident.to_string(), &pt.ty, quote! { &#ident }))
    });
    quote! { ::std::vec![ #(#entries),* ] }
}

/// Wrap an already-`component_scope`d body (see `wrap_component_body_new_core`)
/// with the registration bracket. No-op unless the body is exactly the
/// one-expression `component_scope(…)` wrapper that fn produced.
pub(crate) fn wrap_body(item_fn: &mut ItemFn, legacy_props: bool, bind_to_injected: bool) {
    if item_fn.block.stmts.len() != 1 {
        return;
    }
    let name = item_fn.sig.ident.to_string();
    let props = props_expr(item_fn, legacy_props, bind_to_injected);
    let Some(syn::Stmt::Expr(scoped, None)) = item_fn.block.stmts.pop() else {
        unreachable!("checked: one statement");
    };
    let register: syn::Stmt = syn::parse_quote! {
        let __idealyst_inspect = ::runtime_core::__inspect::__inspect_component(
            #name,
            ::core::file!(),
            ::core::line!(),
            || {
                #[allow(unused_imports)]
                use ::runtime_core::__inspect::probe::*;
                #props
            },
        );
    };
    // Built as AST with the parsed body MOVED in. The obvious
    // `parse_quote! { __idealyst_inspect.finish(#scoped) }` prints the whole
    // body back to tokens and parses it again — measured on CrewForge's
    // projects crate that re-parse was the largest single phase of
    // `#[component]` expansion (~35%), paid on every hot-patch replay. Same
    // AST, same call-site spans as the template would have produced.
    let finish = syn::Expr::MethodCall(syn::ExprMethodCall {
        attrs: Vec::new(),
        receiver: Box::new(syn::parse_quote!(__idealyst_inspect)),
        dot_token: Default::default(),
        method: syn::Ident::new("finish", proc_macro2::Span::call_site()),
        turbofish: None,
        paren_token: Default::default(),
        args: std::iter::once(scoped).collect(),
    });
    item_fn.block.stmts.push(register);
    item_fn.block.stmts.push(syn::Stmt::Expr(finish, None));
}

/// `impl InspectProps for Struct` for a `#[props]` struct — one probe per
/// named field, reading `self.field`. Empty for a generic struct (see
/// [`props_expr`]).
pub(crate) fn inspect_props_impl(input: &syn::DeriveInput, fields: &[(syn::Ident, Type)]) -> TokenStream2 {
    if !input.generics.params.is_empty() {
        return TokenStream2::new();
    }
    let ident = &input.ident;
    let entries = fields
        .iter()
        .map(|(name, ty)| probe_entry(&name.to_string(), ty, quote! { &self.#name }));
    quote! {
        impl ::runtime_core::__inspect::InspectProps for #ident {
            fn __inspect_props(&self) -> ::std::vec::Vec<::runtime_core::__inspect::PropEntry> {
                #[allow(unused_imports)]
                use ::runtime_core::__inspect::probe::*;
                ::std::vec![ #(#entries),* ]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ty(src: &str) -> Type {
        syn::parse_str(src).unwrap()
    }

    #[test]
    fn render_type_keeps_idents_whole() {
        assert_eq!(render_type(&ty("i32")), "i32");
        assert_eq!(render_type(&ty("Vec<String>")), "Vec<String>");
        assert_eq!(render_type(&ty("&'a str")), "&'a str");
        assert_eq!(render_type(&ty("Rc<dyn Fn(i32, bool)>")), "Rc<dyn Fn(i32, bool)>");
        assert_eq!(
            render_type(&ty("::runtime_core::Reactive<Option<String>>")),
            "Reactive<Option<String>>",
            "framework path prefixes are the macro's spelling, not the author's"
        );
    }

    fn wrapped(src: &str, legacy: bool) -> String {
        let mut f: ItemFn = syn::parse_str(src).unwrap();
        wrap_body(&mut f, legacy, false);
        quote!(#f).to_string()
    }

    #[test]
    fn inline_params_each_get_a_probe() {
        let out = wrapped(
            "fn Badge(label: Reactive<String>, count: Reactive<i32>) -> Element { \
             ::runtime_core::component_scope(move || { body() }) }",
            false,
        );
        assert!(out.contains("__inspect_component"), "{out}");
        assert!(out.contains("\"label\""), "{out}");
        assert!(out.contains("\"Reactive<String>\""), "{out}");
        assert!(out.contains("\"count\""), "{out}");
        assert!(out.contains("__idealyst_inspect . finish"), "{out}");
    }

    #[test]
    fn legacy_props_read_through_the_struct() {
        let out = wrapped(
            "fn counter(props: &CounterProps) -> Element { \
             ::runtime_core::component_scope(move || { body() }) }",
            true,
        );
        assert!(out.contains("PropsProbe (& props)"), "{out}");
        assert!(!out.contains("PropProbe (& props)"), "{out}");
    }

    #[test]
    fn generic_components_register_without_props() {
        let out = wrapped(
            "fn List<T: Clone>(items: Vec<T>) -> Element { \
             ::runtime_core::component_scope(move || { body() }) }",
            false,
        );
        assert!(out.contains("__inspect_component"), "{out}");
        assert!(!out.contains("PropProbe"), "{out}");
    }

    #[test]
    fn a_body_that_is_not_the_scope_wrapper_is_left_alone() {
        let out = wrapped("fn Weird() -> Element { let x = 1; x }", false);
        assert!(!out.contains("__inspect_component"), "{out}");
    }
}
