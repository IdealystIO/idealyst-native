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
    "Signal", "ReadSignal", "WriteSignal", "Reactive", "Rx", // imperative handles
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

    let literals = apply_literal_impl(&input.ident, &collect_fields(&input));
    quote! {
        #input
        #literals
    }
}

/// `(field name, post-wrap field type)` for every named field.
fn collect_fields(input: &DeriveInput) -> Vec<(syn::Ident, Type)> {
    let Data::Struct(data) = &input.data else { return Vec::new() };
    let Fields::Named(fields) = &data.fields else { return Vec::new() };
    fields
        .named
        .iter()
        .filter_map(|f| f.ident.clone().map(|i| (i, f.ty.clone())))
        .collect()
}

/// Which descriptor literal a field's type can accept.
enum LitTarget {
    /// A string literal, via `Into` from `&str`.
    Str,
    /// An integer literal, cast to this type then `Into`.
    Int(Type),
    /// A float literal, same shape.
    Float(Type),
    Bool,
    /// Nothing a generated method can build.
    None,
}

/// Classify a (possibly `Reactive`-wrapped) prop type.
///
/// One `Reactive<T>` layer is looked through, because reactive-by-default
/// is the *wrapping*, not the prop's value shape: `label: String` becomes
/// `Reactive<String>` and a descriptor's `"x"` still has to land in it.
/// `Option<T>` is deliberately NOT looked through: assigning would need
/// `Some(v).into()`, and an optional prop with a literal default is rare
/// enough that the site-local fallback in the template emission covers it.
fn literal_target(ty: &Type) -> LitTarget {
    let inner = reactive_inner(ty).unwrap_or(ty);
    let Type::Path(tp) = inner else { return LitTarget::None };
    let Some(seg) = tp.path.segments.last() else { return LitTarget::None };
    if !seg.arguments.is_empty() {
        return LitTarget::None;
    }
    match seg.ident.to_string().as_str() {
        "String" | "str" => LitTarget::Str,
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64"
        | "u128" | "usize" => LitTarget::Int(inner.clone()),
        "f32" | "f64" => LitTarget::Float(inner.clone()),
        "bool" => LitTarget::Bool,
        _ => LitTarget::None,
    }
}

/// The `T` in `Reactive<T>` (however the path is qualified).
fn reactive_inner(ty: &Type) -> Option<&Type> {
    let Type::Path(tp) = ty else { return None };
    let seg = tp.path.segments.last()?;
    if seg.ident != "Reactive" {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &seg.arguments else { return None };
    args.args.iter().find_map(|a| match a {
        GenericArgument::Type(t) => Some(t),
        _ => None,
    })
}

/// Generate `Props::__apply_literal`.
///
/// The template lowering's builder cannot assign an arbitrarily-typed
/// field, so a component node's LITERAL props are carried in the
/// descriptor as data and applied by name through this method. That is
/// what makes a literal-prop edit a data change rather than a
/// recompile.
///
/// Shape: `fn __apply_literal(&mut self, name: &str, value: &LiteralValue)
/// -> bool`, returning whether it applied. Taking the value by REFERENCE
/// (the one deviation from the originally-specified signature) avoids
/// cloning a `Cow` per prop per build; nothing else changes.
///
/// It is an INHERENT method, and that is load-bearing: the builder calls
/// it as `props.__apply_literal(…)` with
/// `runtime_core::__template::ApplyLiteralFallback` in scope, whose
/// blanket impl answers `false` for every type. Rust resolves inherent
/// methods before trait methods, so a `#[props]` / `#[component]` struct
/// uses this one and a props type without the macro silently falls back —
/// which is the contract: such a component is slot-only under the
/// template lowering, never an error.
///
/// `LiteralValue::Path` (an enum-like path such as `tone::Danger`) gets
/// no arm ON PURPOSE. A generated method cannot construct an arbitrary
/// variant of an arbitrary type from its source text without a
/// `FromStr`-shaped bound on every prop type, which would be an API
/// change on every component in the tree. The template emission keeps a
/// site-local resolver for those instead.
pub(crate) fn apply_literal_impl(
    ty: &syn::Ident,
    fields: &[(syn::Ident, Type)],
) -> TokenStream2 {
    // A `children: Vec<Element>` field, if this struct has one. Matched
    // by NAME and by the field's outer type being a `Vec`: the type is
    // the contract `ui!` already enforces at every call site with a
    // children block, so nothing new is being assumed here.
    let children_arm = match fields.iter().find(|(name, ty)| name == "children" && is_vec(ty)) {
        Some((name, _)) => quote! {
            self.#name = children;
            true
        },
        None => quote! { false },
    };

    let arms = fields.iter().filter_map(|(name, field_ty)| {
        let key = name.to_string();
        match literal_target(field_ty) {
            LitTarget::Str => Some(quote! {
                (#key, ::runtime_core::__template::TemplateLiteral::Str(__v)) => {
                    self.#name = (&**__v).into();
                    true
                }
            }),
            LitTarget::Int(t) => Some(quote! {
                (#key, ::runtime_core::__template::TemplateLiteral::Int(__v)) => {
                    self.#name = ((*__v) as #t).into();
                    true
                }
            }),
            LitTarget::Float(t) => Some(quote! {
                (#key, ::runtime_core::__template::TemplateLiteral::Float(__v)) => {
                    self.#name = ((*__v) as #t).into();
                    true
                }
            }),
            LitTarget::Bool => Some(quote! {
                (#key, ::runtime_core::__template::TemplateLiteral::Bool(__v)) => {
                    self.#name = (*__v).into();
                    true
                }
            }),
            LitTarget::None => None,
        }
    });
    quote! {
        #[automatically_derived]
        impl #ty {
            /// Apply one template-descriptor literal by prop name.
            /// Generated; see `runtime_macros::props_attr`.
            #[doc(hidden)]
            #[allow(unused_variables, clippy::all)]
            pub fn __apply_literal(
                &mut self,
                name: &str,
                value: &::runtime_core::__template::TemplateLiteral,
            ) -> bool {
                match (name, value) {
                    #(#arms)*
                    _ => false,
                }
            }
        }

        #[automatically_derived]
        impl #ty {
            /// Move a patch-built child list into this props struct's
            /// `children` field, reporting whether it has one.
            ///
            /// The other half of `__apply_literal`, for the same reason
            /// and by the same mechanism: an overlay that is asked to
            /// INSERT a component has to fill its children, and only
            /// code generated on the concrete type can name the field.
            /// A props struct without a `children: Vec<Element>` field
            /// gets the blanket-trait `false`, and inserting that
            /// component with children is refused rather than silently
            /// dropping them.
            #[doc(hidden)]
            #[allow(unused_variables, unused_mut, clippy::all)]
            pub fn __apply_children(
                &mut self,
                children: ::std::vec::Vec<::runtime_core::Element>,
            ) -> bool {
                #children_arm
            }
        }
    }
}

/// Whether a field's outer type is a `Vec<…>`.
fn is_vec(ty: &Type) -> bool {
    matches!(ty, Type::Path(tp) if tp.path.segments.last().is_some_and(|s| s.ident == "Vec"))
}

/// Default-wrap with a skip-list: returns true unless the type is a known
/// non-reactive-data shape. Syntactic (the macro has tokens, not resolved
/// types) — the same heuristic class as `.get()`-sniffing; a type alias
/// hiding a skip-shape slips through and is corrected with `#[prop(static)]`.
/// Shared with `inline_props` so inline fn parameters wrap by the same rule.
pub(crate) fn should_wrap(ty: &Type) -> bool {
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
    fn skips_read_and_write_signal_halves() {
        // The capability halves are already reactive sources — wrapping
        // one in `Reactive<…>` would be as meaningless as wrapping a
        // `Signal` (and would break `.get()` in the component body).
        let out = rendered(quote! {
            struct P {
                items: ReadSignal<Vec<Row>>,
                report: WriteSignal<f32>,
            }
        });
        assert!(out.contains("items:ReadSignal<Vec<Row>>"), "{out}");
        assert!(out.contains("report:WriteSignal<f32>"), "{out}");
        assert!(!out.contains("Reactive"), "halves must stay unwrapped: {out}");
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
        // `#[prop(...)]`, not the substring "prop" — the generated
        // `__apply_literal` mentions "prop name" in its docs.
        assert!(!out.contains("#[prop"), "the #[prop] attr must be stripped: {out}");
    }

    #[test]
    fn prop_reactive_forces_wrap() {
        // A skip-shape (Vec) the author wants reactive anyway.
        let out = rendered(quote! {
            struct P { #[prop(reactive)] items: Vec<Row> }
        });
        assert!(out.contains("items:::runtime_core::Reactive<Vec<Row>>"), "{out}");
    }

    // -------------------------------------------------------------------
    // __apply_literal
    // -------------------------------------------------------------------

    /// One arm per literal-capable field, keyed on the prop NAME — which
    /// is how the template builder addresses it (the descriptor carries
    /// names, not positions).
    #[test]
    fn apply_literal_generates_one_arm_per_literal_capable_field() {
        let out = rendered(quote! {
            struct P { name: String, count: i32, ratio: f32, on: bool }
        });
        assert!(out.contains(r#"("name",::runtime_core::__template::TemplateLiteral::Str(__v))"#), "{out}");
        assert!(out.contains(r#"("count",::runtime_core::__template::TemplateLiteral::Int(__v))"#), "{out}");
        assert!(out.contains(r#"("ratio",::runtime_core::__template::TemplateLiteral::Float(__v))"#), "{out}");
        assert!(out.contains(r#"("on",::runtime_core::__template::TemplateLiteral::Bool(__v))"#), "{out}");
    }

    /// `Reactive<T>` is looked through: reactive-by-default is the
    /// WRAPPING, not the prop's value shape, so a descriptor's `"x"`
    /// still has to land in a `Reactive<String>`. The `.into()` at the
    /// assignment is what bridges them, exactly as at a `ui!` call site.
    #[test]
    fn apply_literal_looks_through_the_reactive_wrap() {
        let out = rendered(quote! { struct P { name: String } });
        assert!(out.contains("name:::runtime_core::Reactive<String>"), "{out}");
        assert!(out.contains("self.name=(&**__v).into();"), "{out}");
    }

    /// An integer literal is cast to the field's own width before the
    /// `.into()`: the descriptor carries one `i64`, the prop may be any
    /// integer type.
    #[test]
    fn apply_literal_casts_to_the_field_width() {
        let out = rendered(quote! { struct P { #[prop(static)] count: u8 } });
        assert!(out.contains("self.count=((*__v)asu8).into();"), "{out}");
    }

    /// A type a generated method cannot build gets NO arm, and the
    /// method answers `false` — which is what routes the value to the
    /// template emission's site-local fallback. `Path` literals (enum
    /// values) are the main case: constructing an arbitrary variant from
    /// its source text would need a `FromStr`-shaped bound on every prop
    /// type.
    #[test]
    fn apply_literal_refuses_what_it_cannot_build() {
        let out = rendered(quote! {
            struct P {
                #[prop(static)] tone: ToneRef,
                children: Vec<Element>,
                on_press: Rc<dyn Fn()>,
                maybe: Option<String>,
            }
        });
        assert!(out.contains("match(name,value){_=>false,}"), "{out}");
        // No `Path` arm is ever generated.
        assert!(!out.contains("TemplateLiteral::Path"), "{out}");
    }

    /// It must be an INHERENT method: the builder calls it as
    /// `props.__apply_literal(…)` with a blanket-impl trait in scope, and
    /// inherent-before-trait resolution is what makes a props type
    /// WITHOUT the macro fall back silently instead of failing to
    /// compile.
    #[test]
    fn apply_literal_is_an_inherent_method() {
        let out = rendered(quote! { struct P { name: String } });
        assert!(out.contains("implP{"), "must be an inherent impl, not a trait impl: {out}");
        assert!(out.contains("pubfn__apply_literal(&mutself,name:&str,"), "{out}");
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
