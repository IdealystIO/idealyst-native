//! `#[derive(DocControls)]` proc macro.
//!
//! Reflects on the annotated struct's fields and generates an
//! `impl DocControls for T` that:
//!
//! 1. Defines a sibling `T_State` struct holding one `Signal<U>`
//!    per controllable field.
//! 2. Implements `init_state()` using `Default::default()` for each
//!    field's underlying value type.
//! 3. Implements `from_state(state)` reading the signals and
//!    populating a fresh `T` value (un-controllable fields fall
//!    back to `Default::default()`).
//! 4. Implements `render_controls(state)` emitting a `controls_panel`
//!    of `control_row` entries, one per controllable field,
//!    dispatching to the appropriate helper in `idea_ui::doc_controls`
//!    based on the field's type.
//!
//! Type → control dispatch:
//!
//! | Field type matches            | Control                       |
//! |-------------------------------|-------------------------------|
//! | `String`                      | `string_control`              |
//! | `bool`                        | `bool_control`                |
//! | `Option<String>`              | `optional_string_control`     |
//! | `Rc<dyn Intent>`              | `intent_control` + `IntentKind` round-trip |
//! | Anything else implementing `VariantEnum` | `variant_enum_control` (by-type fallback) |
//! | Other (`Rc<dyn Fn…>`, etc.)   | skipped — `Default::default()` |
//!
//! The type-matching is **syntactic** — we look at the field's
//! AST and match `Option`, `Rc`, `dyn Intent`, etc. by token shape.
//! Anything we don't recognize falls through to the
//! variant-enum-or-skip path; the generated code uses
//! `VariantEnum::all_variants()` to decide whether to render
//! controls, falling back to "no control" if the trait isn't
//! implemented.
//!
//! Per-field attributes:
//! - `#[doc_control(skip)]` — omit the field from controls; use
//!   `Default::default()` for its value.
//! - `#[doc_control(label = "Custom Label")]` — override the
//!   control's label (default: humanized field name).

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Data, DeriveInput, Fields, GenericArgument, PathArguments, Type};

#[proc_macro_derive(DocControls, attributes(doc_control))]
pub fn derive_doc_controls(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let struct_name = &input.ident;
    let state_name = format_ident!("{}State", struct_name);

    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(named) => named.named.iter().collect::<Vec<_>>(),
            _ => {
                return syn::Error::new_spanned(
                    struct_name,
                    "DocControls only supports structs with named fields",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(struct_name, "DocControls only supports structs")
                .to_compile_error()
                .into();
        }
    };

    let mut state_fields: Vec<TokenStream2> = Vec::new();
    let mut init_field_inits: Vec<TokenStream2> = Vec::new();
    // For each *controllable* field, a `props.field = state.field.get();`
    // statement. `from_state` starts from `Default::default()` of the
    // whole props struct and overwrites these. That neatly skips
    // callback / `Rc<dyn Fn>` / `Ref<H>` fields which don't have
    // their own `Default` impl but are reachable through the
    // struct's own `Default`.
    let mut from_state_overrides: Vec<TokenStream2> = Vec::new();
    let mut control_rows: Vec<TokenStream2> = Vec::new();
    // For the `reactive_preview` helper: each controllable field
    // contributes a `.get()` call in the scrutinee tuple. The
    // tuple's `PartialEq` lets `switch` decide when to rebuild.
    let mut key_reads: Vec<TokenStream2> = Vec::new();

    for field in fields {
        let f_ident = field.ident.as_ref().expect("named");
        let f_label = field_label(field, &f_ident.to_string());
        let skip = field_is_skipped(field);
        let kind = classify_field_type(&field.ty);

        if skip {
            // Skipped — no state field, no controls, no override.
            // The whole-struct `Default::default()` fills this in.
            continue;
        }

        match kind {
            FieldKind::String => {
                state_fields.push(quote! {
                    pub #f_ident: ::runtime_core::Signal<::std::string::String>
                });
                init_field_inits.push(quote! {
                    #f_ident: ::runtime_core::Signal::new(::std::string::String::default())
                });
                from_state_overrides.push(quote! {
                    props.#f_ident = state.#f_ident.get();
                });
                control_rows.push(quote! {
                    ::idea_ui::doc_controls::control_row(
                        #f_label,
                        ::idea_ui::doc_controls::string_control(state.#f_ident),
                    )
                });
                key_reads.push(quote! { state.#f_ident.get() });
            }
            FieldKind::Bool => {
                state_fields.push(quote! {
                    pub #f_ident: ::runtime_core::Signal<bool>
                });
                init_field_inits.push(quote! {
                    #f_ident: ::runtime_core::Signal::new(false)
                });
                from_state_overrides.push(quote! {
                    props.#f_ident = state.#f_ident.get();
                });
                control_rows.push(quote! {
                    ::idea_ui::doc_controls::control_row(
                        #f_label,
                        ::idea_ui::doc_controls::bool_control(state.#f_ident),
                    )
                });
                key_reads.push(quote! { state.#f_ident.get() });
            }
            FieldKind::OptionString => {
                let enabled_field = format_ident!("__{}_enabled", f_ident);
                state_fields.push(quote! {
                    pub #enabled_field: ::runtime_core::Signal<bool>
                });
                state_fields.push(quote! {
                    pub #f_ident: ::runtime_core::Signal<::std::string::String>
                });
                init_field_inits.push(quote! {
                    #enabled_field: ::runtime_core::Signal::new(false)
                });
                init_field_inits.push(quote! {
                    #f_ident: ::runtime_core::Signal::new(::std::string::String::default())
                });
                from_state_overrides.push(quote! {
                    props.#f_ident = ::idea_ui::doc_controls::optional_string_value(
                        state.#enabled_field,
                        state.#f_ident,
                    );
                });
                control_rows.push(quote! {
                    ::idea_ui::doc_controls::control_row(
                        #f_label,
                        ::idea_ui::doc_controls::optional_string_control(
                            state.#enabled_field,
                            state.#f_ident,
                        ),
                    )
                });
                key_reads.push(quote! { state.#enabled_field.get() });
                key_reads.push(quote! { state.#f_ident.get() });
            }
            FieldKind::Intent => {
                // Legacy `Rc<dyn Intent>` props are no longer the
                // public surface — component props now hold a
                // typed `IntentTag` enum, which dispatches through
                // the `VariantEnum` path below. If a custom
                // component still uses `Rc<dyn Intent>`, the panel
                // skips it; the field falls through to
                // `Default::default()`.
            }
            FieldKind::VariantEnum(ty) => {
                state_fields.push(quote! {
                    pub #f_ident: ::runtime_core::Signal<#ty>
                });
                init_field_inits.push(quote! {
                    #f_ident: ::runtime_core::Signal::new(
                        <#ty as ::core::default::Default>::default(),
                    )
                });
                from_state_overrides.push(quote! {
                    props.#f_ident = state.#f_ident.get();
                });
                control_rows.push(quote! {
                    ::idea_ui::doc_controls::control_row(
                        #f_label,
                        ::idea_ui::doc_controls::variant_enum_control(state.#f_ident),
                    )
                });
                key_reads.push(quote! { state.#f_ident.get() });
            }
            FieldKind::RefHandle(ty) => {
                // Same shape as VariantEnum but routed through the
                // generic `ref_picker_control`, which uses the
                // `RefBuiltins` trait impl on the *Ref type.
                state_fields.push(quote! {
                    pub #f_ident: ::runtime_core::Signal<#ty>
                });
                init_field_inits.push(quote! {
                    #f_ident: ::runtime_core::Signal::new(
                        <#ty as ::core::default::Default>::default(),
                    )
                });
                from_state_overrides.push(quote! {
                    props.#f_ident = state.#f_ident.get();
                });
                control_rows.push(quote! {
                    ::idea_ui::doc_controls::control_row(
                        #f_label,
                        ::idea_ui::doc_controls::ref_picker_control::<#ty>(state.#f_ident),
                    )
                });
                // Ref handles don't implement PartialEq directly —
                // hash the current_key instead for the resolution
                // cache key.
                key_reads.push(quote! {
                    <#ty as ::idea_theme::extensible::RefBuiltins>::current_key(
                        &state.#f_ident.get()
                    ).to_string()
                });
            }
            FieldKind::Unknown => {
                // Fields we can't reflect on don't appear in the
                // state or controls — the whole-struct
                // `Default::default()` covers them.
            }
        }
    }

    let expanded = quote! {
        /// Auto-generated by `#[derive(DocControls)]`. Holds one
        /// signal per controllable field; the docs page constructs
        /// it once and twiddles its signals through the rendered
        /// controls panel.
        ///
        /// `Copy + Clone` because every field is a `Signal<T>`
        /// (id-based, trivially copyable). Makes it cheap to
        /// capture the whole state by-value into the
        /// `reactive_preview` build closure.
        #[allow(non_camel_case_types)]
        #[derive(::std::clone::Clone, ::std::marker::Copy)]
        pub struct #state_name {
            #( #state_fields, )*
        }

        impl ::idea_ui::doc_controls::DocControls for #struct_name {
            type State = #state_name;

            fn init_state() -> Self::State {
                Self::State {
                    #( #init_field_inits, )*
                }
            }

            fn from_state(state: &Self::State) -> Self {
                let mut props = <Self as ::core::default::Default>::default();
                #( #from_state_overrides )*
                props
            }

            fn render_controls(state: &Self::State) -> ::runtime_core::Primitive {
                let rows: ::std::vec::Vec<::runtime_core::Primitive> = vec![
                    #( #control_rows, )*
                ];
                ::idea_ui::doc_controls::controls_panel(rows)
            }

            fn reactive_preview<F: ::std::ops::Fn(Self) -> ::runtime_core::Primitive + 'static>(
                state: &Self::State,
                build: F,
            ) -> ::runtime_core::Primitive {
                // Copy the state out (it's just Signal handles).
                // The scrutinee + branch both close over `state` —
                // copying lets each have its own owned snapshot.
                let state = *state;
                ::runtime_core::switch(
                    move || ( #( #key_reads, )* ),
                    move |_key| {
                        let props = <Self as ::idea_ui::doc_controls::DocControls>::from_state(&state);
                        build(props)
                    },
                )
            }
        }
    };

    expanded.into()
}

// =============================================================================
// Field-type classification
// =============================================================================

enum FieldKind {
    String,
    Bool,
    OptionString,
    /// `Rc<dyn Intent>` — uses the hardcoded built-in intent picker.
    Intent,
    /// A type expected to implement `VariantEnum`. The generated
    /// code uses `all_variants()` on it.
    VariantEnum(Type),
    /// A `*Ref` newtype handle from `idea_theme::extensible`
    /// (`ToneRef`, `VariantRef`, `ButtonSizeRef`, `ShapeRef`,
    /// `TypographyKindRef`, or any app-defined `*Ref` implementing
    /// `idea_theme::extensible::RefBuiltins`). Generated code uses
    /// the generic `ref_picker_control` to enumerate built-ins.
    RefHandle(Type),
    /// Type we don't have a control for. Field is filled by
    /// `Default::default()` and gets no panel entry.
    Unknown,
}

fn classify_field_type(ty: &Type) -> FieldKind {
    if is_type_path(ty, &["String", "string::String", "std::string::String"]) {
        return FieldKind::String;
    }
    if is_type_path(ty, &["bool"]) {
        return FieldKind::Bool;
    }
    if let Some(inner) = extract_path_generic_single(ty, "Option") {
        if is_type_path(&inner, &["String", "string::String", "std::string::String"]) {
            return FieldKind::OptionString;
        }
    }
    if is_rc_dyn_intent(ty) {
        return FieldKind::Intent;
    }
    // *Ref newtype handles (ToneRef, VariantRef, ButtonSizeRef,
    // ShapeRef, TypographyKindRef, or app-defined). Detected by name
    // suffix — the type must implement `idea_theme::extensible::RefBuiltins`
    // for the generated control to compile, which the suffix
    // convention enforces by surface contract.
    if let Some(ident) = simple_path_ident(ty) {
        let s = ident.to_string();
        if s.ends_with("Ref") && s != "Ref" {
            return FieldKind::RefHandle(ty.clone());
        }
    }
    // Variant-enum fallback: simple path types whose name ends in a
    // suffix the stylesheet macro generates (look like
    // `*Size`, `*Kind`, `*Tone`, `*Axis`, `*Padding`, `*Align`,
    // `*Justify`, `*Gap`, `*Tag` — the latter covers `IntentTag`,
    // the per-component "which intent" picker). The generated
    // `VariantEnum` impl exists for these by convention. Anything
    // else falls through to Unknown so we don't get cryptic
    // trait-bound errors.
    if let Some(ident) = simple_path_ident(ty) {
        let s = ident.to_string();
        if [
            "Size",
            "Kind",
            "Tone",
            "Axis",
            "Padding",
            "Align",
            "Justify",
            "Gap",
            "Tag",
            "Color",
        ]
        .iter()
        .any(|suffix| s.ends_with(suffix))
        {
            return FieldKind::VariantEnum(ty.clone());
        }
    }
    FieldKind::Unknown
}

/// If `ty` is a single-ident path (no generics, no qualifications),
/// return that ident. Used to filter "this is probably a
/// stylesheet-generated variant enum" by name pattern.
fn simple_path_ident(ty: &Type) -> Option<syn::Ident> {
    let Type::Path(p) = ty else {
        return None;
    };
    if p.qself.is_some() {
        return None;
    }
    if p.path.segments.len() != 1 {
        return None;
    }
    let seg = &p.path.segments[0];
    if !matches!(seg.arguments, PathArguments::None) {
        return None;
    }
    Some(seg.ident.clone())
}

/// Check if the type is an `Rc<dyn Intent>`. Tolerant of paths
/// like `std::rc::Rc<dyn Intent>` or `Rc<dyn idea_ui::Intent>`.
fn is_rc_dyn_intent(ty: &Type) -> bool {
    let Some(inner) = extract_path_generic_single(ty, "Rc") else {
        return false;
    };
    let Type::TraitObject(t) = &inner else {
        return false;
    };
    for bound in &t.bounds {
        if let syn::TypeParamBound::Trait(tb) = bound {
            // Last segment ident is the trait name.
            if let Some(seg) = tb.path.segments.last() {
                if seg.ident == "Intent" {
                    return true;
                }
            }
        }
    }
    false
}

fn is_type_path(ty: &Type, options: &[&str]) -> bool {
    let Type::Path(p) = ty else {
        return false;
    };
    if p.qself.is_some() {
        return false;
    }
    // Compose the path as a colon-joined string of segment names.
    let s: Vec<String> = p
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    let joined = s.join("::");
    options.iter().any(|o| *o == joined)
}

/// If `ty` is `Wrapper<Inner>` (one generic arg, simple path),
/// return `Inner`.
fn extract_path_generic_single(ty: &Type, wrapper: &str) -> Option<Type> {
    let Type::Path(p) = ty else { return None };
    let last = p.path.segments.last()?;
    if last.ident != wrapper {
        return None;
    }
    let PathArguments::AngleBracketed(ab) = &last.arguments else {
        return None;
    };
    let mut iter = ab.args.iter();
    let first = iter.next()?;
    if iter.next().is_some() {
        return None;
    }
    let GenericArgument::Type(inner) = first else {
        return None;
    };
    Some(inner.clone())
}

// =============================================================================
// Attribute parsing — `#[doc_control(skip)]`, `#[doc_control(label = "…")]`
// =============================================================================

fn field_is_skipped(field: &syn::Field) -> bool {
    for attr in &field.attrs {
        if !attr.path().is_ident("doc_control") {
            continue;
        }
        let mut skip = false;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                skip = true;
            } else {
                // Skip unknown idents (parsed for their side
                // effects by other attribute handlers, e.g.
                // `label`). We don't error here since the field
                // can have multiple doc_control args.
                let _ = meta.value().and_then(|v| v.parse::<syn::Expr>());
            }
            Ok(())
        });
        if skip {
            return true;
        }
    }
    false
}

fn field_label(field: &syn::Field, fallback: &str) -> String {
    for attr in &field.attrs {
        if !attr.path().is_ident("doc_control") {
            continue;
        }
        let mut out: Option<String> = None;
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("label") {
                let value = meta.value()?;
                let lit: syn::LitStr = value.parse()?;
                out = Some(lit.value());
            }
            Ok(())
        });
        if let Some(s) = out {
            return s;
        }
    }
    humanize_ident(fallback)
}

fn humanize_ident(s: &str) -> String {
    // `on_click` -> "On click"; `padding_horizontal` -> "Padding horizontal".
    let mut out = String::with_capacity(s.len());
    let mut next_upper = true;
    for c in s.chars() {
        if c == '_' {
            out.push(' ');
            next_upper = false;
            continue;
        }
        if next_upper {
            for u in c.to_uppercase() {
                out.push(u);
            }
            next_upper = false;
        } else {
            out.push(c);
        }
    }
    out
}
