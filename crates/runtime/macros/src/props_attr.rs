//! `#[props]` — reactive-by-default props struct rewriter.
//!
//! `ui!` lowers a component call `Foo(bar = x)` to a struct literal
//! `FooProps { bar: (x).into(), .. }`, so a prop's *liveness* is decided by
//! the field's declared TYPE: a plain `T` flattens any value to a snapshot,
//! while `Reactive<T>` carries a `Signal`/`rx!` through live (see
//! `reactive_value.rs`). `#[props]` makes reactive the DEFAULT: it rewrites
//! each scalar-data field `T` → `Reactive<T>` so the call site can pass a
//! signal/`rx!` without the component author hand-wrapping every field.
//!
//! ## What gets wrapped
//!
//! Default is **wrap**. Wrapping is skipped for shapes that aren't
//! reactive *data* (a `Reactive<Rc<dyn Fn()>>` is meaningless — handlers
//! aren't sink-consumed; children/refs have their own reactivity):
//!
//! - handlers / callbacks: `Rc`/`Arc`/`Box<dyn Fn…>`, bare `fn(…)`
//! - children / elements: `Element`, `Vec<…>`, `ChildList`
//! - imperative handles: `Ref`, `Bound`, `Bindable`, `RefFill`, `Action`
//! - reactive sources already: `Signal`, `Reactive`, `Rx` (idempotent —
//!   never double-wraps to `Reactive<Reactive<T>>`)
//!
//! `Option<Inner>` is looked through: `Option<String>` →
//! `Reactive<Option<String>>`, but `Option<Rc<dyn Fn…>>` is left alone.
//!
//! ## Overrides
//!
//! Per-field `#[prop(static)]` forces a bare `T` (a genuinely build-time
//! value, or a non-`Clone` type), and `#[prop(reactive)]` forces the wrap
//! (correcting a heuristic miss, e.g. a type alias hiding a data type).
//! Both attributes are stripped before the struct is re-emitted.
//!
//! `#[props]` must sit ABOVE the derives so it rewrites the field types
//! before `#[derive(IdealystSchema)]` / `Default` see them.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{
    parse2, Data, DeriveInput, Fields, GenericArgument, PathArguments, PathSegment, Type,
};

/// Outer type idents that are NOT reactive *data* — never wrapped.
const SKIP: &[&str] = &[
    // handlers
    "Rc", "Arc", "Box", // (only matter when wrapping `dyn Fn`, but a
    // smart-pointer prop is virtually always a handler/shared resource —
    // wrapping it in Reactive is never what's wanted; override with
    // `#[prop(reactive)]` in the rare case)
    // reactive sources (idempotent)
    "Signal", "Reactive", "Rx", // imperative handles
    "Ref", "Bound", "Bindable", "RefFill", "Action", // children / collections
    "Element", "ChildList", "Vec", "HashMap", "BTreeMap", "HashSet",
    // misc non-data
    "PhantomData",
];

pub(crate) fn emit(item: TokenStream2) -> TokenStream2 {
    let mut input: DeriveInput = match parse2(item) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error(),
    };

    let Data::Struct(data) = &mut input.data else {
        return syn::Error::new_spanned(&input.ident, "#[props] only applies to structs")
            .to_compile_error();
    };
    let Fields::Named(fields) = &mut data.fields else {
        return syn::Error::new_spanned(
            &input.ident,
            "#[props] requires a struct with named fields",
        )
        .to_compile_error();
    };

    for field in fields.named.iter_mut() {
        // Read + strip the `#[prop(static|reactive)]` override.
        let mut forced: Option<bool> = None;
        field.attrs.retain(|a| {
            if a.path().is_ident("prop") {
                if let Ok(list) = a.meta.require_list() {
                    match list.tokens.to_string().trim() {
                        "static" => forced = Some(false),
                        "reactive" => forced = Some(true),
                        _ => {}
                    }
                }
                false // strip — `prop` isn't a real attribute
            } else {
                true
            }
        });

        let wrap = forced.unwrap_or_else(|| should_wrap(&field.ty));
        if wrap {
            let ty = &field.ty;
            field.ty = parse2(quote! { ::runtime_core::Reactive<#ty> })
                .expect("Reactive wrap produced an invalid type");
        }
    }

    quote! { #input }
}

/// Default-wrap with a skip-list: returns true unless the type is a known
/// non-reactive-data shape. Syntactic (the macro has tokens, not resolved
/// types) — the same heuristic class as `.get()`-sniffing; a type alias
/// hiding a skip-shape slips through and is corrected with `#[prop(static)]`.
fn should_wrap(ty: &Type) -> bool {
    match ty {
        Type::Path(tp) => {
            let Some(seg) = tp.path.segments.last() else {
                return true;
            };
            let name = seg.ident.to_string();
            if SKIP.contains(&name.as_str()) {
                return false;
            }
            if name == "Option" {
                // Look through to the inner type: `Option<String>` wraps,
                // `Option<Rc<dyn Fn…>>` does not.
                return option_inner(seg).map(should_wrap).unwrap_or(true);
            }
            true
        }
        // Bare function pointers / references / tuples / etc. are never data.
        _ => false,
    }
}

/// The `T` in `Option<T>`.
fn option_inner(seg: &PathSegment) -> Option<&Type> {
    if let PathArguments::AngleBracketed(args) = &seg.arguments {
        for arg in &args.args {
            if let GenericArgument::Type(t) = arg {
                return Some(t);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn rendered(input: TokenStream2) -> String {
        emit(input).to_string().chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn wraps_scalar_data_fields() {
        let out = rendered(quote! {
            struct P { flag: bool, name: String, size: FieldSize }
        });
        assert!(out.contains("flag:::runtime_core::Reactive<bool>"), "{out}");
        assert!(out.contains("name:::runtime_core::Reactive<String>"), "{out}");
        assert!(out.contains("size:::runtime_core::Reactive<FieldSize>"), "{out}");
    }

    #[test]
    fn looks_through_option() {
        let out = rendered(quote! { struct P { label: Option<String> } });
        assert!(out.contains("label:::runtime_core::Reactive<Option<String>>"), "{out}");
    }

    #[test]
    fn skips_handlers_children_refs_and_sources() {
        let out = rendered(quote! {
            struct P {
                on_change: Rc<dyn Fn(String)>,
                children: Vec<Element>,
                handle: Ref<H>,
                live: Signal<i32>,
            }
        });
        assert!(out.contains("on_change:Rc<dynFn(String)>"), "{out}");
        assert!(out.contains("children:Vec<Element>"), "{out}");
        assert!(out.contains("handle:Ref<H>"), "{out}");
        assert!(out.contains("live:Signal<i32>"), "{out}");
        assert!(!out.contains("Reactive"), "no data field to wrap: {out}");
    }

    #[test]
    fn is_idempotent_on_reactive() {
        let out = rendered(quote! { struct P { x: Reactive<bool> } });
        // Must NOT become Reactive<Reactive<bool>>.
        assert!(out.contains("x:Reactive<bool>"), "{out}");
        assert!(!out.contains("Reactive<Reactive"), "{out}");
    }

    #[test]
    fn prop_static_forces_bare() {
        let out = rendered(quote! {
            struct P { #[prop(static)] size: FieldSize }
        });
        assert!(out.contains("size:FieldSize"), "{out}");
        assert!(!out.contains("Reactive"), "static override must not wrap: {out}");
        assert!(!out.contains("prop"), "the #[prop] attr must be stripped: {out}");
    }

    #[test]
    fn prop_reactive_forces_wrap() {
        // A skip-shape (Vec) the author wants reactive anyway.
        let out = rendered(quote! {
            struct P { #[prop(reactive)] items: Vec<Row> }
        });
        assert!(out.contains("items:::runtime_core::Reactive<Vec<Row>>"), "{out}");
    }

    #[test]
    fn preserves_other_field_attrs() {
        let out = rendered(quote! {
            struct P { #[schema(constraint = "x")] name: String }
        });
        assert!(out.contains("schema"), "non-prop attrs must survive: {out}");
        assert!(out.contains("name:::runtime_core::Reactive<String>"), "{out}");
    }
}
