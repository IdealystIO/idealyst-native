//! Inline-props `#[component]` — component signatures that declare props
//! as ordinary fn parameters, Leptos-style, instead of a hand-written
//! props struct:
//!
//! ```ignore
//! #[component]
//! fn Badge(
//!     /// Text shown inside the badge.
//!     label: String,
//!     #[prop(default = 3)] count: i32,
//!     #[prop(static)] size: BadgeSize,
//!     on_press: Option<Rc<dyn Fn()>>,
//! ) -> Element { ... }
//! ```
//!
//! expands to the exact same contract the two-declaration form produces —
//! a `BadgeProps` struct (fields wrapped reactive-by-default with the same
//! rules as `#[props]`), a `Default` impl carrying the per-arg defaults, a
//! `pub type Badge = BadgeProps` tag alias, and a `BuildElement` impl —
//! so `ui! { Badge(label = "hi") }` dispatches identically. The fn itself
//! keeps its parameters (by value, types rewritten to `Reactive<T>` where
//! wrapped); `build()` calls it field-by-field. Keeping the params in the
//! signature (rather than destructuring a struct in a prelude) is what
//! lets `reactivity::rewrite`'s parameter-rooted-path cloning keep working
//! unchanged: each prop IS a parameter, so bare `label` in a reactive
//! closure gets the same clone-at-boundary treatment `props.label` gets in
//! the legacy form.
//!
//! Recognized per-arg attributes (`#[prop(...)]`, comma-separable):
//! - `default = expr` — value when the call site omits the prop. Coerced
//!   with `.into()` against the (possibly wrapped) field type, same as
//!   `#[component(default(...))]`. A field with no declared default falls
//!   back to `Default::default()` — which is why a bare `Rc<dyn Fn()>`
//!   prop needs either an explicit default (`Rc::new(|| {}) as Rc<dyn
//!   Fn()>`) or, better, the `Option<Rc<dyn Fn()>>` shape.
//! - `static` / `reactive` — force the wrap decision, same as on
//!   `#[props]` struct fields.
//! - `optional` / `into` — accepted no-ops for Leptos parity: every
//!   idealyst prop is already optional (the `..defaults()` struct-update
//!   base), and every `ui!` prop value is already coerced via `.into()`.
//!
//! Doc comments on a parameter move onto the generated struct field, so
//! hovering the prop at a `ui!` call site shows them. (rustc never sees
//! the param attrs — they're stripped before the fn is re-emitted.)
//!
//! ## What stays on the legacy path
//!
//! - Zero-parameter components (empty marker struct, `invocation_macro`).
//! - The classic single-struct signature: one parameter that is either
//!   named `props` or typed `[&]XxxProps`, with no `#[prop]` attrs.
//! - Generic components (a generated struct can't carry the fn's generics
//!   through `BuildElement`, which is object-simple by design).

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::{Attribute, Expr, FnArg, Ident, ItemFn, Pat, Token, Type};

use crate::component_attr::ComponentAttr;
use crate::props_attr::should_wrap;

/// One generated props-struct field, harvested from a fn parameter.
struct Field {
    name: Ident,
    /// Post-wrap type (what the struct field AND the rewritten parameter
    /// share).
    ty: Type,
    docs: Vec<Attribute>,
    default: Option<Expr>,
}

/// Detects the inline-props shape and, when it applies, rewrites the fn
/// signature in place (strips `#[prop]`/doc attrs from params, wraps
/// param types) and returns the generated dispatch glue. `Ok(None)`
/// means "legacy shape — use `invocation_macro` as before".
pub(crate) fn try_expand(
    item_fn: &mut ItemFn,
    attr: &ComponentAttr,
) -> syn::Result<Option<TokenStream2>> {
    if item_fn.sig.inputs.is_empty() {
        // Zero-parameter components stay on the legacy path (empty marker
        // struct via `invocation_macro`) — EXCEPT lazy ones: the chunk glue
        // needs the generated inline-props struct to carry the `loading` /
        // `error` config fields, and an empty parameter list is trivially
        // the inline shape (a props struct with only those two fields).
        // Zero-arg lazy components are common — a route screen or a heavy
        // SDK corner takes no input.
        if attr.lazy && item_fn.sig.generics.params.is_empty() {
            return Ok(Some(emit_glue(item_fn, &[], attr)));
        }
        return Ok(None);
    }
    if is_legacy_props_sig(&item_fn.sig) {
        return Ok(None);
    }
    // Generic components keep the legacy behavior (their sig can't have
    // been the inline shape before this feature existed, but a generated
    // monomorphic struct can't carry the fn's generics — fall through so
    // the author gets the same "no dispatch glue" outcome as today).
    if !item_fn.sig.generics.params.is_empty() {
        return Ok(None);
    }

    // Inline mode owns per-arg defaults; a second, struct-level default
    // channel would make the same prop's default declarable in two
    // places.
    if let Some(first) = attr.defaults.first() {
        return Err(syn::Error::new(
            first.name.span(),
            "#[component(default(...))] cannot be combined with inline props; \
             declare the default on the parameter instead: `#[prop(default = ...)] name: Ty`",
        ));
    }

    let mut fields: Vec<Field> = Vec::new();
    for input in item_fn.sig.inputs.iter_mut() {
        let pt = match input {
            FnArg::Receiver(r) => {
                return Err(syn::Error::new_spanned(
                    r,
                    "components are free functions; `self` is not a valid component parameter",
                ));
            }
            FnArg::Typed(pt) => pt,
        };
        let Pat::Ident(pat_ident) = &*pt.pat else {
            return Err(syn::Error::new_spanned(
                &pt.pat,
                "inline component props must be plain identifiers \
                 (destructuring patterns can't name a props-struct field)",
            ));
        };
        let name = pat_ident.ident.clone();

        // Partition the param's attributes: docs move to the struct
        // field, `#[prop(...)]` is consumed here, anything else stays on
        // the param (rustc will judge it).
        let mut docs = Vec::new();
        let mut prop = PropArgAttr::default();
        let mut kept = Vec::new();
        for a in pt.attrs.drain(..) {
            if a.path().is_ident("doc") {
                docs.push(a);
            } else if a.path().is_ident("prop") {
                prop.absorb(&a)?;
            } else {
                kept.push(a);
            }
        }
        pt.attrs = kept;

        // Same reactive-by-default rule as `#[props]` struct fields; the
        // parameter's type is rewritten so the body sees what the struct
        // stores.
        let wrap = prop.forced_wrap.unwrap_or_else(|| should_wrap(&pt.ty));
        if wrap {
            let inner = (*pt.ty).clone();
            *pt.ty = syn::parse_quote! { ::runtime_core::Reactive<#inner> };
        }

        fields.push(Field {
            name,
            ty: (*pt.ty).clone(),
            docs,
            default: prop.default,
        });
    }

    Ok(Some(emit_glue(item_fn, &fields, attr)))
}

/// The classic explicit-props signature: exactly one parameter, no
/// `#[prop]` attrs, and either the parameter is named `props` or its
/// type's last path segment ends in `Props` (`&FooProps` / `FooProps`).
pub(crate) fn is_legacy_props_sig(sig: &syn::Signature) -> bool {
    if sig.inputs.len() != 1 {
        return false;
    }
    let FnArg::Typed(pt) = &sig.inputs[0] else {
        return false;
    };
    if pt.attrs.iter().any(|a| a.path().is_ident("prop")) {
        return false;
    }
    if let Pat::Ident(pi) = &*pt.pat {
        if pi.ident == "props" {
            return true;
        }
    }
    let ty = match &*pt.ty {
        Type::Reference(r) => &*r.elem,
        other => other,
    };
    if let Type::Path(p) = ty {
        if let Some(seg) = p.path.segments.last() {
            return seg.ident.to_string().ends_with("Props");
        }
    }
    false
}

/// Emits the props struct + `Default` + tag alias + `BuildElement` impl.
/// Mirrors `invocation_macro::generate_build_impl` output shape; the only
/// structural difference is `build()` calling the fn field-by-field and
/// the per-field defaults living directly in `Default` (so no
/// `defaults()` override is needed).
fn emit_glue(item_fn: &ItemFn, fields: &[Field], attr: &ComponentAttr) -> TokenStream2 {
    let fn_name = &item_fn.sig.ident;
    let vis = &item_fn.vis;
    let props_ident = format_ident!("{}Props", fn_name);

    // The component's doc comment rides on the tag alias, same as the
    // legacy path, so hovering the `ui!` tag is useful.
    let fn_docs: Vec<&Attribute> =
        item_fn.attrs.iter().filter(|a| a.path().is_ident("doc")).collect();
    let struct_doc = format!(
        "Props for [`{fn_name}`]. Generated by `#[component]` from the fn's inline parameters."
    );

    let field_defs = fields.iter().map(|f| {
        let Field { name, ty, docs, .. } = f;
        quote! { #(#docs)* pub #name: #ty, }
    });
    let default_fields = fields.iter().map(|f| {
        let name = &f.name;
        match &f.default {
            // `.into()` pinned by the field type — the same coercion
            // `#[component(default(...))]` and the `ui!` call site apply.
            Some(expr) => quote! { #name: (#expr).into(), },
            // `new-core`: `runtime_world`'s signal handles have no
            // `Default` (the old core's detached-sentinel Default can't
            // be reproduced for a foreign type). A signal-typed prop's
            // struct-update base mints a fresh default-valued signal in
            // the ambient scope instead (`glue::__default_signal_prop`,
            // reached through the retargeted path). Divergence, on
            // purpose and documented (glue docs): OMITTING a required
            // signal prop reads a fresh default-valued signal here,
            // where the old core panicked on first read.
            None if cfg!(feature = "new-core") && is_signal_handle_type(&f.ty) => {
                quote! { #name: ::runtime_core::__default_signal_prop(), }
            }
            None => quote! { #name: ::core::default::Default::default(), },
        }
    });

    if attr.lazy {
        // Lazy mode: the component's body compiles into a separate wasm chunk
        // and `build()` yields an `Element::Lazy`. The struct gains `loading` /
        // `error` config fields (not forwarded to the component fn), and the
        // component's args cross the chunk boundary as `props`.
        let arg_names: Vec<&Ident> = fields.iter().map(|f| &f.name).collect();
        return crate::lazy_component::emit_inline_lazy_glue(crate::lazy_component::LazyGlue {
            fn_name,
            props_ident: &props_ident,
            vis,
            fn_docs: &fn_docs,
            struct_doc: &struct_doc,
            field_defs: field_defs.collect(),
            default_fields: default_fields.collect(),
            arg_names: &arg_names,
            retryable: attr.retryable,
        });
    }

    let build_args = fields.iter().map(|f| {
        let name = &f.name;
        quote! { self.#name }
    });

    quote! {
        #[doc = #struct_doc]
        #[allow(non_camel_case_types)]
        #vis struct #props_ident {
            #(#field_defs)*
        }

        #[automatically_derived]
        impl ::core::default::Default for #props_ident {
            fn default() -> Self {
                Self { #(#default_fields)* }
            }
        }

        #(#fn_docs)*
        #[allow(non_camel_case_types)]
        #vis type #fn_name = #props_ident;

        #[automatically_derived]
        impl ::runtime_core::BuildElement for #props_ident {
            fn build(self) -> ::runtime_core::Element {
                // Coerce via `IntoElement` so a component returning a
                // richer type than bare `Element` still satisfies the
                // trait — same as the legacy impl.
                ::runtime_core::IntoElement::into_element(#fn_name(#(#build_args),*))
            }
        }
    }
}

/// Syntactic check: is this prop type one of the reactive HANDLE types
/// (`Signal<T>` / `ReadSignal<T>` / `WriteSignal<T>`, optionally
/// qualified)? Same heuristic class as `props_attr::should_wrap` — a
/// type alias hiding a handle slips through and surfaces as a normal
/// `Default` bound error.
fn is_signal_handle_type(ty: &Type) -> bool {
    let Type::Path(tp) = ty else { return false };
    tp.path
        .segments
        .last()
        .is_some_and(|seg| matches!(seg.ident.to_string().as_str(), "Signal" | "ReadSignal" | "WriteSignal"))
}

/// Accumulated `#[prop(...)]` overrides for one parameter. Multiple
/// `#[prop]` attrs merge.
#[derive(Default)]
struct PropArgAttr {
    default: Option<Expr>,
    forced_wrap: Option<bool>,
}

impl PropArgAttr {
    fn absorb(&mut self, attr: &Attribute) -> syn::Result<()> {
        let list = attr.meta.require_list()?;
        let parsed: PropArgList = syn::parse2(list.tokens.clone())?;
        for entry in parsed.entries {
            match entry {
                PropArgEntry::Default(span, expr) => {
                    if self.default.is_some() {
                        return Err(syn::Error::new(span, "duplicate `default` on this prop"));
                    }
                    self.default = Some(expr);
                }
                PropArgEntry::Static => self.forced_wrap = Some(false),
                PropArgEntry::Reactive => self.forced_wrap = Some(true),
                PropArgEntry::Noop => {}
            }
        }
        Ok(())
    }
}

enum PropArgEntry {
    Default(proc_macro2::Span, Expr),
    Static,
    Reactive,
    /// `optional` / `into` — Leptos-parity spellings whose semantics are
    /// already the unconditional behavior here.
    Noop,
}

struct PropArgList {
    entries: Vec<PropArgEntry>,
}

impl Parse for PropArgList {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut entries = Vec::new();
        while !input.is_empty() {
            // `parse_any` because `static` is a keyword.
            let ident = input.call(Ident::parse_any)?;
            let entry = match ident.to_string().as_str() {
                "default" => {
                    let _: Token![=] = input.parse()?;
                    PropArgEntry::Default(ident.span(), input.parse()?)
                }
                "static" => PropArgEntry::Static,
                "reactive" => PropArgEntry::Reactive,
                "optional" | "into" => PropArgEntry::Noop,
                other => {
                    return Err(syn::Error::new(
                        ident.span(),
                        format!(
                            "unknown prop argument `{other}`; supported: \
                             `default = expr`, `static`, `reactive`, `optional`, `into`"
                        ),
                    ));
                }
            };
            entries.push(entry);
            if input.is_empty() {
                break;
            }
            let _: Token![,] = input.parse()?;
        }
        Ok(PropArgList { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component_attr::parse_component_attr;
    use quote::quote;

    fn expand(fn_tokens: TokenStream2) -> (ItemFn, Option<String>) {
        let mut item_fn: ItemFn = syn::parse2(fn_tokens).unwrap();
        let attr = parse_component_attr(TokenStream2::new()).unwrap();
        let glue = try_expand(&mut item_fn, &attr).unwrap();
        (item_fn, glue.map(|g| squash(g)))
    }

    fn squash(t: TokenStream2) -> String {
        t.to_string().chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn legacy_props_struct_sig_is_left_alone() {
        for sig in [
            quote! { fn Card(props: &CardProps) -> Element { body() } },
            quote! { fn Card(props: CardProps) -> Element { body() } },
            // Param named `props` counts as legacy even without the
            // `Props` type suffix.
            quote! { fn Card(props: &CardArgs) -> Element { body() } },
        ] {
            let (_, glue) = expand(sig);
            assert!(glue.is_none(), "legacy shape must not expand inline");
        }
    }

    #[test]
    fn zero_params_left_alone() {
        let (_, glue) = expand(quote! { fn Spacer() -> Element { body() } });
        assert!(glue.is_none());
    }

    #[test]
    fn single_inline_param_expands() {
        let (item_fn, glue) =
            expand(quote! { fn Badge(label: String) -> Element { body() } });
        let glue = glue.expect("inline shape must expand");
        // Struct field + param share the wrapped type.
        assert!(glue.contains("publabel:::runtime_core::Reactive<String>"), "{glue}");
        let sig = squash(quote! { #item_fn });
        assert!(sig.contains("label:::runtime_core::Reactive<String>"), "{sig}");
        // Dispatch glue: alias + BuildElement calling the fn field-wise.
        assert!(glue.contains("typeBadge=BadgeProps"), "{glue}");
        assert!(glue.contains("Badge(self.label)"), "{glue}");
    }

    #[test]
    fn multi_param_field_order_preserved() {
        let (_, glue) = expand(quote! {
            fn Row(a: String, b: i32) -> Element { body() }
        });
        let glue = glue.unwrap();
        assert!(glue.contains("Row(self.a,self.b)"), "{glue}");
    }

    #[test]
    fn default_lands_in_default_impl_with_into() {
        let (_, glue) = expand(quote! {
            fn Badge(#[prop(default = 3)] count: i32) -> Element { body() }
        });
        let glue = glue.unwrap();
        assert!(glue.contains("count:(3).into()"), "{glue}");
    }

    #[test]
    fn missing_default_falls_back_to_default_default() {
        let (_, glue) = expand(quote! { fn Badge(label: String) -> Element { body() } });
        let glue = glue.unwrap();
        assert!(glue.contains("label:::core::default::Default::default()"), "{glue}");
    }

    #[test]
    fn prop_static_keeps_bare_type() {
        let (item_fn, glue) = expand(quote! {
            fn Chip(#[prop(static)] size: u8, label: String) -> Element { body() }
        });
        let glue = glue.unwrap();
        assert!(glue.contains("pubsize:u8"), "{glue}");
        let sig = squash(quote! { #item_fn });
        assert!(sig.contains("size:u8"), "{sig}");
        assert!(!sig.contains("prop"), "the #[prop] attr must be stripped: {sig}");
    }

    /// Signal-typed props can't use `Default::default()` on the new
    /// core (`runtime_world::Signal` has no Default); the glue helper
    /// mints the struct-update base instead.
    #[cfg(feature = "new-core")]
    #[test]
    fn new_core_signal_props_default_via_glue_helper() {
        let (_, glue) = expand(quote! {
            fn Foo(value: Signal<i32>, out: WriteSignal<bool>) -> Element { body() }
        });
        let glue = glue.unwrap();
        assert_eq!(glue.matches("__default_signal_prop").count(), 2, "{glue}");
        assert!(!glue.contains("value:::core::default::Default::default()"), "{glue}");
    }

    /// …and on the OLD core the plain `Default::default()` base is
    /// unchanged (the old `Signal` has the detached-sentinel Default).
    #[cfg(not(feature = "new-core"))]
    #[test]
    fn old_core_signal_props_default_via_default_impl() {
        let (_, glue) = expand(quote! {
            fn Foo(value: Signal<i32>) -> Element { body() }
        });
        let glue = glue.unwrap();
        assert!(glue.contains("value:::core::default::Default::default()"), "{glue}");
        assert!(!glue.contains("__default_signal_prop"), "{glue}");
    }

    #[test]
    fn skip_shapes_stay_unwrapped() {
        let (_, glue) = expand(quote! {
            fn Frame(children: Vec<Element>, value: Signal<i32>, on_press: Option<Rc<dyn Fn()>>) -> Element { body() }
        });
        let glue = glue.unwrap();
        assert!(glue.contains("pubchildren:Vec<Element>"), "{glue}");
        assert!(glue.contains("pubvalue:Signal<i32>"), "{glue}");
        assert!(glue.contains("pubon_press:Option<Rc<dynFn()>>"), "{glue}");
    }

    #[test]
    fn optional_and_into_are_accepted_noops() {
        let (_, glue) = expand(quote! {
            fn Badge(#[prop(optional, into)] label: String) -> Element { body() }
        });
        assert!(glue.unwrap().contains("publabel:::runtime_core::Reactive<String>"));
    }

    #[test]
    fn param_docs_move_to_field() {
        let (item_fn, glue) = expand(quote! {
            fn Badge(#[doc = " the label"] label: String) -> Element { body() }
        });
        assert!(glue.unwrap().contains("doc=\"thelabel\""));
        let sig = squash(quote! { #item_fn });
        assert!(!sig.contains("thelabel"), "param docs must be stripped: {sig}");
    }

    #[test]
    fn component_level_default_rejected_in_inline_mode() {
        let mut item_fn: ItemFn =
            syn::parse2(quote! { fn Badge(count: i32) -> Element { body() } }).unwrap();
        let attr = parse_component_attr(quote! { default(count = 3) }).unwrap();
        let err = try_expand(&mut item_fn, &attr).unwrap_err();
        assert!(err.to_string().contains("prop(default"), "{err}");
    }

    #[test]
    fn destructuring_param_rejected() {
        let mut item_fn: ItemFn = syn::parse2(quote! {
            fn Badge((a, b): (i32, i32)) -> Element { body() }
        })
        .unwrap();
        let attr = parse_component_attr(TokenStream2::new()).unwrap();
        let err = try_expand(&mut item_fn, &attr).unwrap_err();
        assert!(err.to_string().contains("plain identifiers"), "{err}");
    }

    #[test]
    fn unknown_prop_key_rejected() {
        let mut item_fn: ItemFn = syn::parse2(quote! {
            fn Badge(#[prop(bogus)] label: String) -> Element { body() }
        })
        .unwrap();
        let attr = parse_component_attr(TokenStream2::new()).unwrap();
        let err = try_expand(&mut item_fn, &attr).unwrap_err();
        assert!(err.to_string().contains("bogus"), "{err}");
    }

    #[test]
    fn generic_fn_left_alone() {
        let (_, glue) = expand(quote! {
            fn Badge<T: Clone>(value: T) -> Element { body() }
        });
        assert!(glue.is_none(), "generic components stay on the legacy path");
    }

    // ------------------------------------------------------------------
    // `#[component(lazy)]` emission battery (lazy_component.rs's glue,
    // reached through try_expand). The shared shape is pinned once; the
    // `__lazy_body` chunk fn differs by core and is pinned per leg.
    // ------------------------------------------------------------------

    fn expand_lazy(attr_tokens: TokenStream2, fn_tokens: TokenStream2) -> String {
        let mut item_fn: ItemFn = syn::parse2(fn_tokens).unwrap();
        let attr = parse_component_attr(attr_tokens).unwrap();
        let glue = try_expand(&mut item_fn, &attr)
            .unwrap()
            .expect("lazy components always take the inline glue path");
        squash(glue)
    }

    #[test]
    fn lazy_emits_wasm_split_boundary_and_config_fields() {
        let glue = expand_lazy(
            quote! { lazy },
            quote! { fn Panel(id: u32) -> Element { body() } },
        );
        // Chunk boundary: the readable split-module name + the attribute
        // path both cores share (retargeted under new-core).
        assert!(glue.contains("wasm_split(__idealyst_lazy_Panel)"), "{glue}");
        assert!(
            glue.contains("use::runtime_core::__wasm_splitaswasm_split"),
            "{glue}"
        );
        // The generated props struct gains the loading/error config slots.
        assert!(
            glue.contains("publoading:::runtime_core::primitives::lazy::LazyLoadingUi"),
            "{glue}"
        );
        assert!(
            glue.contains("puberror:::runtime_core::primitives::lazy::LazyErrorUi"),
            "{glue}"
        );
        // One-shot loader (no `retryable`): take-once cell, loud on reuse.
        assert!(glue.contains("invokedtwicewithout`retryable`"), "{glue}");
        assert!(!glue.contains("derive(::core::clone::Clone)"), "{glue}");
        // Builder wiring: lazy_split + placeholder/on_error config.
        assert!(
            glue.contains("::runtime_core::primitives::lazy::lazy_split"),
            "{glue}"
        );
    }

    #[test]
    fn lazy_retryable_derives_clone_and_clones_props_per_attempt() {
        let glue = expand_lazy(
            quote! { lazy, retryable },
            quote! { fn Panel(id: u32) -> Element { body() } },
        );
        assert!(glue.contains("derive(::core::clone::Clone)"), "{glue}");
        assert!(glue.contains("::core::clone::Clone::clone(&__props)"), "{glue}");
        assert!(!glue.contains("invokedtwicewithout"), "{glue}");
    }

    #[test]
    fn lazy_zero_params_takes_inline_glue_path() {
        // Zero-arg lazy components are common (route screens); they must
        // NOT fall back to the legacy empty-marker path — the chunk glue
        // needs the generated struct for the loading/error fields.
        let glue = expand_lazy(quote! { lazy }, quote! { fn Screen() -> Element { body() } });
        assert!(glue.contains("wasm_split(__idealyst_lazy_Screen)"), "{glue}");
        assert!(glue.contains("publoading"), "{glue}");
    }

    /// DEFAULT-leg byte stability: the old core's chunk fn constructs the
    /// component's `Element` inside the loader future — the pre-new-core
    /// emission, byte-for-byte.
    #[cfg(not(feature = "new-core"))]
    #[test]
    fn lazy_default_leg_chunk_fn_builds_element() {
        let glue = expand_lazy(
            quote! { lazy },
            quote! { fn Panel(id: u32) -> Element { body() } },
        );
        assert!(
            glue.contains(
                "asyncfn__lazy_body(props:PanelProps)->::runtime_core::Element{\
                 ::runtime_core::IntoElement::into_element(Panel(props.id))}"
            ),
            "{glue}"
        );
        assert!(!glue.contains("LazyBodyThunk"), "{glue}");
    }

    /// NEW-core leg: the chunk fn returns a body THUNK — construction is
    /// deferred to the vocabulary lazy handler's swap effect, which runs
    /// with the world entered inside `component_scope` (a naked executor
    /// poll has no ambient world; building there would panic). See
    /// lazy_component.rs + runtime-vocabulary's `prims::lazy` docs.
    #[cfg(feature = "new-core")]
    #[test]
    fn lazy_new_core_leg_chunk_fn_returns_body_thunk() {
        let glue = expand_lazy(
            quote! { lazy },
            quote! { fn Panel(id: u32) -> Element { body() } },
        );
        assert!(
            glue.contains(
                "asyncfn__lazy_body(props:PanelProps,)\
                 ->::runtime_core::primitives::lazy::LazyBodyThunk"
            ),
            "{glue}"
        );
        // Explicitly-typed thunk binding (NOT a bare `Box::new` tail):
        // the wasm `#[wasm_split]` expansion re-homes the body inside
        // `Box::pin(async move { … })`, whose tail has no coercion
        // context — without the annotation the closure never unsizes to
        // `Box<dyn FnOnce() -> Element>` (caught on the first live
        // wasm32 website build).
        assert!(
            glue.contains(
                "let__thunk:::runtime_core::primitives::lazy::LazyBodyThunk=\
                 ::std::boxed::Box::new(move||{\
                 ::runtime_core::IntoElement::into_element(Panel(props.id))});__thunk"
            ),
            "{glue}"
        );
    }
}
