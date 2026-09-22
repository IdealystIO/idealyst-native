//! The `#[component]` hot-reload split: one component fn becomes an
//! inner `__<Name>_hot_impl` carrying the body and an outer `<Name>`
//! that dispatches to it through the subsecond jump table.
//!
//! # Why a split at all
//!
//! Subsecond rebinds *function addresses*. A dev-time patch dylib is
//! linked from freshly-emitted objects for the edited crate, and
//! `dev_hot::apply_patch` installs a table mapping each old address to
//! its address in the patch. For an entry to do anything, the running
//! program has to reach that function through an INDIRECT call that
//! consults the table — a direct `bl <addr>` in already-linked code
//! cannot be redirected. `dev_hot::call(fn_ptr, args)` is that indirect
//! call, so every component body has to sit behind one.
//!
//! The outer fn keeps the author's name, visibility, signature and
//! attributes exactly, so the SHAPE of a component — the props struct,
//! the `Tag = TagProps` alias, the `BuildElement` impl, every call site
//! — is identical with the feature on or off. The split is internal.
//!
//! # The fn-pointer coercion is load-bearing
//!
//! The body binds the inner fn to an explicitly typed `fn(..) -> ..`
//! local before handing it to `dev_hot::call`. A bare named function in
//! Rust is a zero-sized *fn item* type; passing one directly makes
//! subsecond's `size_of::<F>() == size_of::<fn()>()` check fail, which
//! routes dispatch through the trait-object path keyed on
//! `<F as HotFunction>::call_it` instead of on the user function's own
//! address. Our jump table is built by pairing `__*_hot_impl` SYMBOLS
//! between the host binary and the patch dylib, so dispatch has to take
//! the fn-pointer path (`call_as_ptr`), which looks the table up by the
//! pointer's runtime address. Coercing to an explicit `fn(..)` pointer
//! is what forces that.
//!
//! # Shapes that are refused
//!
//! A component whose signature cannot be spelled as a plain fn pointer
//! is emitted unchanged (see [`Refusal`]). It still compiles, still
//! renders, and a save that touches it falls back to
//! rebuild-and-respawn — it is simply not on the subsecond fast path.
//! Refusing is a value, not a diagnostic: a dev-only accelerator must
//! never fail a build that is fine in production.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Ident, ItemFn};

/// Why a component fn could not be put on the jump-table fast path.
///
/// Every variant names a shape whose signature has no `fn(..) -> ..`
/// spelling (or none that subsecond's `HotFunction` impls cover).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// Generic parameters (type, const or lifetime). The inner fn
    /// would need a turbofish the outer cannot always infer, and each
    /// monomorphization gets its own symbol — the jump table pairs by
    /// name, so a patch that instantiates a different set silently
    /// rebinds nothing. Generic components keep the legacy dispatch
    /// path anyway (`inline_props::try_expand` leaves them alone), so
    /// they are already the exceptional shape.
    Generics,
    /// More parameters than subsecond implements `HotFunction` for
    /// (arity 0..=9). Carries the actual count.
    Arity(usize),
    /// A parameter bound by a pattern that is not a plain identifier
    /// (tuple destructuring, `_`). The outer fn has no name to forward.
    NonIdentParam,
    /// `impl Trait` in argument or return position — unnameable in a
    /// fn-pointer type.
    ImplTrait,
    /// A `self` receiver. Components are free functions; this is
    /// belt-and-braces.
    Receiver,
    /// `async fn` / `unsafe fn` / `extern "C" fn` / C-variadic. None of
    /// these are component shapes, and each changes the callable type.
    NotAPlainFn,
}

/// Maximum component arity the split supports — subsecond implements
/// `HotFunction` for `Fn0Marker..=Fn9Marker`, i.e. up to nine
/// arguments. A wider component is emitted unsplit.
pub(crate) const MAX_SPLIT_ARITY: usize = 9;

/// Split `item_fn` into `(inner impl, outer dispatcher)`.
///
/// On refusal the fn is handed back untouched alongside the reason, so
/// the caller can emit it verbatim.
pub(crate) fn split(item_fn: ItemFn) -> Result<TokenStream2, (Refusal, Box<ItemFn>)> {
    if let Some(r) = refuse(&item_fn) {
        return Err((r, Box::new(item_fn)));
    }

    let outer_name = item_fn.sig.ident.clone();
    let inner_name = Ident::new(&format!("__{}_hot_impl", outer_name), outer_name.span());

    // The inner fn: the author's signature, the fully-rewritten body,
    // a private name. `#[inline(never)]` keeps it a real symbol the
    // jump table can pair — an inlined-away body has no address to
    // rebind, and the patch dylib would contain no `__*_hot_impl` for
    // it. Doc comments are dropped (the outer keeps them; two copies
    // would duplicate every component in rustdoc).
    let mut inner = item_fn.clone();
    inner.vis = syn::Visibility::Inherited;
    inner.sig.ident = inner_name.clone();
    inner.attrs.retain(|a| !a.path().is_ident("doc"));
    inner.attrs.retain(|a| !a.path().is_ident("inline"));
    inner.attrs.push(syn::parse_quote!(#[doc(hidden)]));
    inner.attrs.push(syn::parse_quote!(#[inline(never)]));
    inner.attrs.push(syn::parse_quote!(#[allow(non_snake_case)]));

    // The outer: same signature, body replaced by the dispatch.
    let mut outer = item_fn;
    // Never `#[inline]` the dispatcher — inlining it into a caller
    // would not break correctness (the table lookup is still dynamic),
    // but it duplicates the lookup at every call site for no gain.
    outer.attrs.retain(|a| !a.path().is_ident("inline"));

    let arg_idents = arg_idents(&outer.sig);
    let arg_types: Vec<&syn::Type> = outer
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pt) => Some(&*pt.ty),
            syn::FnArg::Receiver(_) => None,
        })
        .collect();
    let ret = match &outer.sig.output {
        syn::ReturnType::Default => quote! { () },
        syn::ReturnType::Type(_, t) => quote! { #t },
    };
    // A one-element tuple needs its trailing comma or it is just a
    // parenthesized expression, and subsecond keys `HotFunction` on the
    // tuple's arity.
    let arg_tuple = if arg_idents.is_empty() {
        quote! { () }
    } else {
        quote! { (#(#arg_idents,)*) }
    };

    outer.block = syn::parse_quote! {
        {
            // See the module docs: the explicit `fn(..)` annotation is
            // what routes dispatch through subsecond's fn-POINTER path
            // (`call_as_ptr`), whose table key is this pointer's
            // runtime address — the address our jump-table generator
            // paired by symbol name. A bare fn item would be a ZST and
            // take the trait-object path instead, which our table has
            // no entry for, and the patch would silently do nothing.
            let __idealyst_hot_inner: fn(#(#arg_types),*) -> #ret = #inner_name;
            ::runtime_core::__hot::call(__idealyst_hot_inner, #arg_tuple)
        }
    };
    outer
        .attrs
        .push(syn::parse_quote!(#[allow(clippy::needless_pass_by_value)]));

    Ok(quote! {
        #inner
        #outer
    })
}

/// The first reason `item_fn` cannot be split, if any.
fn refuse(f: &ItemFn) -> Option<Refusal> {
    if !f.sig.generics.params.is_empty() {
        return Some(Refusal::Generics);
    }
    if f.sig.asyncness.is_some()
        || f.sig.unsafety.is_some()
        || f.sig.abi.is_some()
        || f.sig.variadic.is_some()
    {
        return Some(Refusal::NotAPlainFn);
    }
    if f.sig.inputs.len() > MAX_SPLIT_ARITY {
        return Some(Refusal::Arity(f.sig.inputs.len()));
    }
    if let syn::ReturnType::Type(_, ty) = &f.sig.output {
        if mentions_impl_trait(ty) {
            return Some(Refusal::ImplTrait);
        }
    }
    for input in f.sig.inputs.iter() {
        let pt = match input {
            syn::FnArg::Receiver(_) => return Some(Refusal::Receiver),
            syn::FnArg::Typed(pt) => pt,
        };
        if !matches!(&*pt.pat, syn::Pat::Ident(pi) if pi.subpat.is_none()) {
            return Some(Refusal::NonIdentParam);
        }
        if mentions_impl_trait(&pt.ty) {
            return Some(Refusal::ImplTrait);
        }
    }
    None
}

/// The binding idents of `sig`'s parameters, in order. Only called
/// after [`refuse`] has proved every parameter is a plain ident, so the
/// mapping is total — a silent skip here would build a wrong-arity
/// argument tuple.
fn arg_idents(sig: &syn::Signature) -> Vec<Ident> {
    sig.inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pt) => match &*pt.pat {
                syn::Pat::Ident(pi) => Some(pi.ident.clone()),
                _ => None,
            },
            syn::FnArg::Receiver(_) => None,
        })
        .collect()
}

/// True if `ty` contains an `impl Trait` anywhere — including nested
/// (`Option<impl Fn()>`), which is just as unspellable in a fn pointer
/// as the bare form.
fn mentions_impl_trait(ty: &syn::Type) -> bool {
    struct Finder(bool);
    impl<'ast> syn::visit::Visit<'ast> for Finder {
        fn visit_type_impl_trait(&mut self, _: &'ast syn::TypeImplTrait) {
            self.0 = true;
        }
    }
    let mut f = Finder(false);
    syn::visit::Visit::visit_type(&mut f, ty);
    f.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn squash(t: TokenStream2) -> String {
        t.to_string().chars().filter(|c| !c.is_whitespace()).collect()
    }

    fn split_str(src: TokenStream2) -> String {
        squash(split(syn::parse2(src).unwrap()).expect("splittable"))
    }

    fn refusal(src: TokenStream2) -> Refusal {
        split(syn::parse2(src).unwrap()).map(|_| ()).unwrap_err().0
    }

    #[test]
    fn zero_arg_component_dispatches_with_a_unit_tuple() {
        let out = split_str(quote! { pub fn Counter() -> Element { body() } });
        assert!(out.contains("fn__Counter_hot_impl()->Element{body()}"), "{out}");
        assert!(out.contains("let__idealyst_hot_inner:fn()->Element=__Counter_hot_impl;"), "{out}");
        assert!(out.contains("::runtime_core::__hot::call(__idealyst_hot_inner,())"), "{out}");
    }

    /// A one-element argument tuple MUST keep its trailing comma —
    /// `(props)` is a parenthesized expression, not a 1-tuple, and
    /// subsecond selects its `HotFunction` impl on the tuple's arity.
    #[test]
    fn single_arg_component_passes_a_one_tuple_not_a_paren_expr() {
        let out = split_str(quote! { fn Card(props: &CardProps) -> Element { b() } });
        assert!(out.contains("(__idealyst_hot_inner,(props,))"), "{out}");
        assert!(
            out.contains("let__idealyst_hot_inner:fn(&CardProps)->Element=__Card_hot_impl;"),
            "{out}"
        );
    }

    #[test]
    fn inline_props_component_forwards_every_parameter_in_order() {
        let out = split_str(quote! {
            fn Badge(label: Reactive<String>, count: i32, children: Vec<Element>) -> Element { b() }
        });
        assert!(
            out.contains(
                "let__idealyst_hot_inner:fn(Reactive<String>,i32,Vec<Element>)->Element\
                 =__Badge_hot_impl;"
            ),
            "{out}"
        );
        assert!(out.contains("(__idealyst_hot_inner,(label,count,children,))"), "{out}");
    }

    /// The author's name, visibility and signature survive verbatim —
    /// this is the "shape does not change" contract every call site,
    /// the `Tag = TagProps` alias and the `BuildElement` impl rely on.
    #[test]
    fn outer_keeps_name_visibility_docs_and_signature() {
        let out = split_str(quote! {
            /// A card.
            pub(crate) fn Card(props: &CardProps) -> Element { b() }
        });
        assert!(out.contains("pub(crate)fnCard(props:&CardProps)->Element"), "{out}");
        assert!(out.contains("#[doc=r\"Acard.\"]"), "{out}");
        // The inner is private and undocumented; exactly one copy of
        // the doc comment survives.
        assert_eq!(out.matches("#[doc=r\"Acard.\"]").count(), 1, "{out}");
        assert!(out.contains("#[doc(hidden)]"), "{out}");
    }

    /// Without `#[inline(never)]` the optimizer is free to inline the
    /// only (indirect) reference away, leaving no `__*_hot_impl` symbol
    /// for the jump-table generator to pair — the patch would link and
    /// apply, and change nothing.
    #[test]
    fn inner_is_inline_never_so_the_symbol_survives() {
        let out = split_str(quote! { fn A() -> Element { b() } });
        assert!(out.contains("#[inline(never)]"), "{out}");
    }

    #[test]
    fn author_inline_attribute_is_dropped_from_both_halves() {
        let out = split_str(quote! { #[inline] fn A() -> Element { b() } });
        assert!(!out.contains("#[inline]"), "{out}");
    }

    #[test]
    fn generic_component_is_refused() {
        assert_eq!(
            refusal(quote! { fn Badge<T: Clone>(v: T) -> Element { b() } }),
            Refusal::Generics
        );
        assert_eq!(
            refusal(quote! { fn Badge<'a>(v: &'a str) -> Element { b() } }),
            Refusal::Generics
        );
    }

    #[test]
    fn over_wide_component_is_refused_with_its_arity() {
        let args: Vec<TokenStream2> =
            (0..10).map(|i| { let n = Ident::new(&format!("a{i}"), proc_macro2::Span::call_site()); quote!(#n: u32) }).collect();
        assert_eq!(
            refusal(quote! { fn Wide(#(#args),*) -> Element { b() } }),
            Refusal::Arity(10)
        );
        // …and exactly nine is still on the fast path.
        let args: Vec<TokenStream2> =
            (0..9).map(|i| { let n = Ident::new(&format!("a{i}"), proc_macro2::Span::call_site()); quote!(#n: u32) }).collect();
        let out = split_str(quote! { fn Wide(#(#args),*) -> Element { b() } });
        assert!(out.contains("__Wide_hot_impl"), "{out}");
    }

    #[test]
    fn impl_trait_parameter_is_refused() {
        assert_eq!(
            refusal(quote! { fn A(f: impl Fn()) -> Element { b() } }),
            Refusal::ImplTrait
        );
        // Nested is just as unspellable in a fn pointer.
        assert_eq!(
            refusal(quote! { fn A(f: Option<impl Fn()>) -> Element { b() } }),
            Refusal::ImplTrait
        );
    }

    #[test]
    fn impl_trait_return_is_refused() {
        assert_eq!(
            refusal(quote! { fn A() -> impl IntoElement { b() } }),
            Refusal::ImplTrait
        );
    }

    /// `collect_arg_idents` used to SKIP a non-ident parameter, which
    /// built a shorter argument tuple than the inner fn's arity —
    /// a wrong-arity call the author saw as a baffling type error
    /// inside macro-generated code. Refuse the whole split instead.
    #[test]
    fn destructuring_parameter_is_refused_not_silently_dropped() {
        assert_eq!(
            refusal(quote! { fn A((x, y): (u32, u32)) -> Element { b() } }),
            Refusal::NonIdentParam
        );
    }

    #[test]
    fn non_plain_fn_shapes_are_refused() {
        assert_eq!(
            refusal(quote! { async fn A() -> Element { b() } }),
            Refusal::NotAPlainFn
        );
        assert_eq!(
            refusal(quote! { unsafe fn A() -> Element { b() } }),
            Refusal::NotAPlainFn
        );
        assert_eq!(
            refusal(quote! { extern "C" fn A() -> Element { b() } }),
            Refusal::NotAPlainFn
        );
    }

    /// A `mut` binding is still a plain ident — the split forwards it
    /// by name and the inner keeps the `mut`.
    #[test]
    fn mut_binding_parameter_is_still_splittable() {
        let out = split_str(quote! { fn A(mut n: u32) -> Element { b() } });
        assert!(out.contains("fn__A_hot_impl(mutn:u32)"), "{out}");
        assert!(out.contains("(__idealyst_hot_inner,(n,))"), "{out}");
    }
}
