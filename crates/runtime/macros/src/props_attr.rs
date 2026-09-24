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

    let literals = apply_literal_impl(&input.ident, &collect_fields(&input), false);
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
    /// A string literal into an `Option<String>` field, as `Some(…)`.
    OptStr,
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
    // `Option<String>`: what a wrapped literal slot (`Some("…".to_string())`,
    // `Some(String::from("…"))`) feeds. The descriptor carries the string
    // inside the wrapper, and this arm puts the `Some` back.
    if seg.ident == "Option" {
        let PathArguments::AngleBracketed(args) = &seg.arguments else { return LitTarget::None };
        let is_string = args.args.iter().any(|a| {
            matches!(a, GenericArgument::Type(Type::Path(t))
                if t.path.segments.last().is_some_and(|s| s.ident == "String" && s.arguments.is_empty()))
        });
        return if is_string && args.args.len() == 1 { LitTarget::OptStr } else { LitTarget::None };
    }
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
/// A descriptor applier cannot assign an arbitrarily-typed
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
/// overlay, never an error.
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
    // Whether this props type also gets a `BuildElement` impl —
    // `#[component]` and the inline-props form do, a bare `#[props]`
    // struct does not. The overlay's CONSTRUCTOR needs it (it builds
    // an `Element`), so it is generated only for the former; a bare
    // props struct still gets the patch hook.
    buildable: bool,
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
            LitTarget::OptStr => Some(quote! {
                (#key, ::runtime_core::__template::TemplateLiteral::Str(__v)) => {
                    self.#name = ::core::option::Option::Some(
                        ::std::string::String::from(&**__v),
                    )
                    .into();
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
    // The overlay's two per-component entry points. Generated ONCE per
    // props TYPE, and emitted at a `ui!` call site as two short
    // statements — a path and a method call.
    //
    // That split is a measurement, not a preference. Emitting these
    // bodies at every call site instead cost +1.7 s on CrewForge's
    // one-edit rebuild (5.0 s to 6.7 s): the constructor closure alone
    // was +1.1 s of it, because a large app has thousands of component
    // call sites and each one was a fresh closure body to expand,
    // type-check and monomorphize. One body per component definition is
    // the same capability for a fraction of the front end.
    let overlay_impl = overlay_hooks(ty, buildable);

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

        #overlay_impl

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

/// The overlay's per-props-type hooks: bind a build to its site, and
/// construct one of these from literals.
///
/// Feature-gated because both name `runtime_core::__overlay`, which only
/// exists under `ui-overlay` — and the emission that calls them is
/// behind the same gate, so the two are never out of step within one
/// build graph.
#[cfg(feature = "ui-overlay")]
fn overlay_hooks(ty: &syn::Ident, buildable: bool) -> TokenStream2 {
    // A props type with a generated `BuildElement` can be CONSTRUCTED
    // from data; one without (a hand-rolled impl, or a bare `#[props]`
    // struct) falls back to the trait's "no", so the overlay refuses to
    // insert it rather than guessing.
    // `OverlayProps` is a REAL impl, and that is the whole point: a
    // generic rebuild body cannot call an inherent `__apply_literal` —
    // it would bind the blanket fallback and apply nothing. Only a
    // buildable props type gets it, which is also how the call-site
    // probe tells a rebuildable component from a props struct the
    // macros never touched.
    let overlay_props =
        if buildable { overlay_props_impl(&quote!(#ty)) } else { TokenStream2::new() };

    let ctor_path = if buildable {
        quote! { Self::__overlay_ctor }
    } else {
        quote! {
            <Self as ::runtime_core::__template::ApplyLiteralFallback>::__overlay_ctor
        }
    };
    let ctor = if buildable {
        quote! {
            /// Build this component from literal props and children.
            ///
            /// Registered by the call site under the component's TAG, so
            /// an overlay asked to INSERT one can. Returns `None` when
            /// the props struct has no `children` field to put the
            /// children in — refused rather than silently dropping them.
            #[doc(hidden)]
            #[allow(clippy::all)]
            pub fn __overlay_ctor(
                props: &[(&str, &::runtime_core::__template::TemplateLiteral)],
                children: ::std::vec::Vec<::runtime_core::Element>,
            ) -> ::core::option::Option<::runtime_core::Element> {
                let mut __p = <#ty as ::runtime_core::BuildElement>::defaults();
                for (__n, __v) in props {
                    __p.__apply_literal(__n, __v);
                }
                if !children.is_empty() && !__p.__apply_children(children) {
                    return ::core::option::Option::None;
                }
                ::core::option::Option::Some(::runtime_core::BuildElement::build(__p))
            }
        }
    } else {
        TokenStream2::new()
    };
    quote! {
        #[automatically_derived]
        impl #ty {
            /// The overlay's per-component job, run from inside this
            /// type's own `BuildElement::build`: teach the overlay how
            /// to build this component, then apply whatever it has
            /// staged for the node being built.
            ///
            /// It has to happen here and not after, because a
            /// component's props do not survive into the built tree —
            /// this is the last moment they are reachable.
            ///
            /// WHICH node is read from the ambient address `ui!` sets
            /// around the build expression, rather than passed in.
            /// Passing it meant emitting this call at every component
            /// call site in the program, and the trait-method resolution
            /// that goes with it; the ambient pair is two integer
            /// arguments to two free functions instead. See
            /// `runtime_macros`' `ui_overlay` for the measurement.
            ///
            /// Registration is unconditional, so a component built ANY
            /// way — a `ui!` tag, a bare `BuildElement::build`, a
            /// fn-call form — can afterwards be inserted by a patch. The
            /// contract is "any component type this program has built
            /// once", not "any component currently on screen".
            ///
            /// `tag` is the name as WRITTEN at a call site (`Badge`, not
            /// `BadgeProps`) — what a descriptor calls the node, and so
            /// what the constructor must be registered under. The type
            /// cannot know it; the macro that generates this can.
            /// A way to run this component again with new literal
            /// props, when its props type allows one.
            ///
            /// The `Clone` decision is made HERE, once per props type,
            /// by autoref specialization: `(&Probe(self)).rebuilder()`
            /// resolves to the `ViaClone` impl when the bounds hold and
            /// autorefs once more onto a `None` fallback when they do
            /// not. No specialization feature, no bound on
            /// `#[component]` — a bound would break every component
            /// with `children: Vec<Element>` — and a props type that is
            /// not `Clone` still compiles.
            ///
            /// One body per props type, not per call site. The call
            /// site version of this cost +1.2 s on a real app's
            /// one-edit rebuild: trait resolution multiplied by
            /// thousands of sites.
            #[doc(hidden)]
            #[allow(clippy::all)]
            pub fn __overlay_rebuilder(
                &self,
            ) -> ::core::option::Option<::runtime_core::__overlay::Rebuilder> {
                #[allow(unused_imports)]
                use ::runtime_core::__overlay::{ViaClone as _, ViaFallback as _};
                (&::runtime_core::__overlay::Probe(self)).rebuilder()
            }

            #[doc(hidden)]
            #[allow(clippy::all)]
            pub fn __overlay_bind(&mut self, tag: &'static str) {
                ::runtime_core::__overlay::register_ctor(tag, #ctor_path);
                // Hand the ambient frame a copy of these props, before
                // the body consumes them. `exit` attaches it to the
                // element, realize carries it to the mounted node, and
                // a live prop edit runs the component again from it.
                ::runtime_core::__overlay::set_rebuilder(self.__overlay_rebuilder());
                let ::core::option::Option::Some((__site, __node)) =
                    ::runtime_core::__overlay::take_current()
                else {
                    return;
                };
                for (__n, __v) in ::runtime_core::__overlay::staged_props(__site, __node) {
                    self.__apply_literal(&__n, &__v);
                }
            }

            #ctor
        }

        #overlay_props
    }
}

#[cfg(not(feature = "ui-overlay"))]
fn overlay_hooks(_ty: &syn::Ident, _buildable: bool) -> TokenStream2 {
    TokenStream2::new()
}

/// The `OverlayProps` impl a rebuildable props type gets.
///
/// A REAL impl, and that is the whole point: a generic rebuild body
/// cannot call an inherent `__apply_literal` — it would bind the blanket
/// fallback and apply nothing. It is also how the call-site probe tells
/// a rebuildable component from a props struct the macros never touched,
/// since only this emission creates one.
#[cfg(feature = "ui-overlay")]
pub(crate) fn overlay_props_impl(ty: &TokenStream2) -> TokenStream2 {
    quote! {
        #[automatically_derived]
        impl ::runtime_core::__overlay::OverlayProps for #ty {
            fn overlay_apply(
                &mut self,
                name: &str,
                value: &::runtime_core::__template::TemplateLiteral,
            ) -> bool {
                // Inherent-before-trait: a props type the macros
                // generated fields for has its own `__apply_literal` and
                // uses that; one that does not — a marker struct for a
                // no-prop component, a hand-rolled props type — falls
                // back to the blanket impl's "nothing applied". Both
                // are correct, and neither is a compile error.
                #[allow(unused_imports)]
                use ::runtime_core::__template::ApplyLiteralFallback as _;
                self.__apply_literal(name, value)
            }

            fn overlay_build(self) -> ::runtime_core::Element {
                ::runtime_core::BuildElement::build(self)
            }
        }
    }
}

#[cfg(not(feature = "ui-overlay"))]
pub(crate) fn overlay_props_impl(_ty: &TokenStream2) -> TokenStream2 {
    TokenStream2::new()
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
                // An `Option` of anything but `String`: no wrapped-literal
                // slot feeds one.
                maybe: Option<i32>,
            }
        });
        assert!(out.contains("match(name,value){_=>false,}"), "{out}");
        // No `Path` arm is ever generated.
        assert!(!out.contains("TemplateLiteral::Path"), "{out}");
    }

    /// An `Option<String>` field takes a string literal as `Some(…)`: it
    /// is what a wrapped-literal slot (`placeholder = Some("…".to_string())`)
    /// feeds, and the descriptor carries only the string inside the
    /// wrapper. Plain and reactive-wrapped fields alike.
    #[test]
    fn apply_literal_puts_the_some_back_on_an_optional_string() {
        let out = rendered(quote! { struct P { placeholder: Option<String> } });
        assert!(
            out.contains(
                "(\"placeholder\",::runtime_core::__template::TemplateLiteral::Str(__v))=>{self.placeholder=::core::option::Option::Some(::std::string::String::from(&**__v),).into();true}"
            ),
            "{out}"
        );
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

/// The prologue a generated `BuildElement::build` runs before the
/// component's own body.
///
/// Returns `(mut_token, statements)` — the `mut` because the props are
/// taken by value and the patch writes into them.
///
/// Emitted once per props TYPE rather than at each `ui!` call site, and
/// that is deliberate: `__overlay_bind`'s fallback is a blanket
/// `impl<T>`, so resolving the call pulls trait selection in wherever it
/// appears. Once per component is nothing; once per call site, across a
/// large app, was measurable. See `runtime_macros`' `ui_overlay`.
#[cfg(feature = "ui-overlay")]
pub(crate) fn overlay_build_prologue(tag: &syn::Ident) -> (TokenStream2, TokenStream2) {
    let tag_str = tag.to_string();
    (
        quote! { mut },
        quote! {
            #[allow(unused_imports)]
            use ::runtime_core::__template::ApplyLiteralFallback as _;
            self.__overlay_bind(#tag_str);
        },
    )
}

#[cfg(not(feature = "ui-overlay"))]
pub(crate) fn overlay_build_prologue(_tag: &syn::Ident) -> (TokenStream2, TokenStream2) {
    (TokenStream2::new(), TokenStream2::new())
}
