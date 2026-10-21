//! `component_styles! { … }` — a component's styled sections, declared
//! once as named slots.
//!
//! ```ignore
//! component_styles! {
//!     /// The text field.
//!     pub Field<AppTheme> {
//!         /// How the input box is drawn.
//!         axis appearance { #[default] contained, outlined }
//!         slot input {
//!             base(t) { background: t.color.surface(), border_radius: 6.0 }
//!             variant appearance {
//!                 contained(t) { background: t.color.surface_alt() }
//!                 outlined(t) { border_width: 1.0, border_color: t.color.border() }
//!             }
//!             variant size { sm(_t) { height: 32.0 } #[default] md(_t) { height: 40.0 } }
//!             state focused(t) { border_color: t.color.primary() }
//!         }
//!         slot label { base(t) { color: t.color.text_muted() } }
//!     }
//! }
//! ```
//!
//! Every `slot` body is the `stylesheet!` grammar (no `override` lines —
//! per-instance overrides are the generated `*SlotStyles`), optionally
//! opened by `extend(path)`: a `fn(&mut SlotRestyle<'_, FieldStyles>)`
//! the library runs at every composition to add computed arms. A
//! component-level `axis name { … }` declares an axis's values and default
//! when no slot spells them all out as arms (arms an `extend` hook
//! computes); every slot `variant` block also contributes its axis.
//!
//! # What it generates (for `Field`)
//!
//! - `FieldStyles` — the registry key, implementing
//!   `runtime_core::ComponentStyles`, with `FieldStyles::input()` /
//!   `::label()` (a `SlotStyle` per slot), `::values()` (a shared
//!   `AxisValues` set), `::restyle(|s| …)` and `::set_default(value)`.
//! - one key newtype per axis — `FieldAppearance`, `FieldSize` — with an
//!   associated const per declared value (`FieldAppearance::Contained`),
//!   `define("pill")` for app values, `Default` (the app's
//!   `set_default`, else the declared default), and `AxisKey` tying it to
//!   `FieldStyles`. Axis values of different components are different
//!   types, so one component's value never fits another's slot.
//! - `FieldRestyle` — what a `restyle` closure receives: `s.input()` /
//!   `s.label()` return the slot's `SlotRestyle`.
//! - `FieldSlotStyles` — the per-instance overrides, one
//!   `Option<SlotOverride>` field (and a same-named builder setter) per
//!   slot; `#[props(styles = FieldStyles)]` adds it to a props struct.
//! - the premint-dump registration that composes every slot after the
//!   dump build ran `app()`.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{braced, parenthesized, Attribute, Ident, Path, Token, Type, Visibility};

use crate::stylesheet::{self, StyleSheetDecl};

pub struct ComponentStylesDecl {
    docs: Vec<Attribute>,
    vis: Visibility,
    name: Ident,
    theme_ty: Type,
    axes: Vec<AxisDecl>,
    slots: Vec<SlotDecl>,
}

struct AxisDecl {
    docs: Vec<Attribute>,
    name: Ident,
    values: Vec<(Vec<Attribute>, Ident)>,
    default: Option<Ident>,
}

struct SlotDecl {
    docs: Vec<Attribute>,
    name: Ident,
    extend: Option<Path>,
    sheet: StyleSheetDecl,
}

/// Names the generated `FieldStyles` impl already uses.
const RESERVED_SLOT_NAMES: &[&str] = &["restyle", "values", "set_default", "slot_names"];

impl Parse for ComponentStylesDecl {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut docs = Vec::new();
        for attr in input.call(Attribute::parse_outer)? {
            if attr.path().is_ident("doc") {
                docs.push(attr);
            } else {
                return Err(syn::Error::new_spanned(
                    attr.path(),
                    "`component_styles!` takes doc comments only on the declaration",
                ));
            }
        }
        let vis: Visibility = input.parse()?;
        let name: Ident = input.parse()?;
        let _lt: Token![<] = input.parse()?;
        let theme_ty: Type = input.parse()?;
        let _gt: Token![>] = input.parse()?;
        let body;
        braced!(body in input);

        let mut axes: Vec<AxisDecl> = Vec::new();
        let mut slots: Vec<SlotDecl> = Vec::new();
        while !body.is_empty() {
            let item_docs = body.call(Attribute::parse_outer)?;
            let kw: Ident = body.parse()?;
            match kw.to_string().as_str() {
                "axis" => {
                    let axis: Ident = body.parse()?;
                    let vals;
                    braced!(vals in body);
                    let mut values: Vec<(Vec<Attribute>, Ident, bool)> = Vec::new();
                    while !vals.is_empty() {
                        let mut vdocs = Vec::new();
                        let mut is_default = false;
                        for attr in vals.call(Attribute::parse_outer)? {
                            if attr.path().is_ident("default") {
                                if values.iter().any(|(_, _, d)| *d) {
                                    return Err(syn::Error::new_spanned(attr, "an axis has one `#[default]`"));
                                }
                                is_default = true;
                            } else if attr.path().is_ident("doc") {
                                vdocs.push(attr);
                            } else {
                                return Err(syn::Error::new_spanned(attr, "only `#[default]` and docs go on an axis value"));
                            }
                        }
                        let v: Ident = vals.parse()?;
                        values.push((vdocs, v, is_default));
                        if vals.is_empty() {
                            break;
                        }
                        let _: Token![,] = vals.parse()?;
                    }
                    if axes.iter().any(|a| a.name == axis) {
                        return Err(syn::Error::new(axis.span(), format!("axis `{axis}` declared twice")));
                    }
                    let default_ident = values.iter().find(|(_, _, d)| *d).map(|(_, v, _)| v.clone());
                    axes.push(AxisDecl {
                        docs: item_docs,
                        name: axis,
                        values: values.into_iter().map(|(d, v, _)| (d, v)).collect(),
                        default: default_ident,
                    });
                }
                "slot" => {
                    let slot: Ident = body.parse()?;
                    if RESERVED_SLOT_NAMES.contains(&slot.to_string().as_str()) {
                        return Err(syn::Error::new(
                            slot.span(),
                            format!("`{slot}` is a generated method on the styles type; name the slot differently"),
                        ));
                    }
                    if slots.iter().any(|s| s.name == slot) {
                        return Err(syn::Error::new(slot.span(), format!("slot `{slot}` declared twice")));
                    }
                    let sbody;
                    braced!(sbody in body);
                    let extend = if sbody.peek(Ident) && sbody.fork().parse::<Ident>()? == "extend" {
                        let _: Ident = sbody.parse()?;
                        let args;
                        parenthesized!(args in sbody);
                        let p: Path = args.parse()?;
                        Some(p)
                    } else {
                        None
                    };
                    let sheet_name = format_ident!("{}{}", name, stylesheet::pascal(&slot));
                    let sheet = stylesheet::parse_sheet_body(
                        Vec::new(),
                        Vec::new(),
                        vis.clone(),
                        sheet_name,
                        theme_ty.clone(),
                        &sbody,
                    )?;
                    if let Some(o) = sheet.overrides.first() {
                        return Err(syn::Error::new(
                            o.name.span(),
                            "a slot has no `override` lines: per-instance overrides are the generated \
                             `*SlotStyles` (one optional override per slot)",
                        ));
                    }
                    slots.push(SlotDecl { docs: item_docs, name: slot, extend, sheet });
                }
                other => {
                    return Err(syn::Error::new(
                        kw.span(),
                        format!("expected `axis` or `slot`, got `{other}`"),
                    ));
                }
            }
        }
        if slots.is_empty() {
            return Err(syn::Error::new(name.span(), "a component declares at least one `slot`"));
        }
        Ok(ComponentStylesDecl { docs, vis, name, theme_ty, axes, slots })
    }
}

/// One merged axis: the component-level declaration (if any) plus every
/// slot arm on it.
struct MergedAxis {
    docs: Vec<Attribute>,
    name: Ident,
    values: Vec<(Vec<Attribute>, Ident)>,
    default: Option<Ident>,
}

fn merge_axes(decl: &ComponentStylesDecl) -> syn::Result<Vec<MergedAxis>> {
    let mut out: Vec<MergedAxis> = decl
        .axes
        .iter()
        .map(|a| MergedAxis {
            docs: a.docs.clone(),
            name: a.name.clone(),
            values: a.values.clone(),
            default: a.default.clone(),
        })
        .collect();
    for slot in &decl.slots {
        for v in &slot.sheet.variants {
            let idx = match out.iter().position(|m| m.name == v.axis) {
                Some(i) => i,
                None => {
                    out.push(MergedAxis { docs: Vec::new(), name: v.axis.clone(), values: Vec::new(), default: None });
                    out.len() - 1
                }
            };
            let m = &mut out[idx];
            for arm in &v.arms {
                if !m.values.iter().any(|(_, x)| *x == arm.name) {
                    m.values.push((Vec::new(), arm.name.clone()));
                }
                if arm.is_default {
                    match &m.default {
                        Some(d) if *d != arm.name => {
                            return Err(syn::Error::new(
                                arm.name.span(),
                                format!(
                                    "axis `{}` defaults to `{d}` elsewhere in `{}`; one axis has one default \
                                     across every slot",
                                    v.axis, decl.name
                                ),
                            ));
                        }
                        _ => m.default = Some(arm.name.clone()),
                    }
                }
            }
        }
    }
    Ok(out)
}

pub fn emit(decl: ComponentStylesDecl, content_hash: u64) -> TokenStream2 {
    let axes = match merge_axes(&decl) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error(),
    };
    let ComponentStylesDecl { docs, vis, name, theme_ty, slots, .. } = &decl;
    let styles = format_ident!("{}Styles", name);
    let restyle_ty = format_ident!("{}Restyle", name);
    let overrides_ty = format_ident!("{}SlotStyles", name);
    let styles_name = styles.to_string();

    // ---- slot sheets ------------------------------------------------------
    let mut slot_defs = Vec::new();
    let mut build_fns = Vec::new();
    let mut accessors = Vec::new();
    let mut restyle_methods = Vec::new();
    let mut override_fields = Vec::new();
    let mut override_setters = Vec::new();
    for (i, slot) in slots.iter().enumerate() {
        let slot_name = &slot.name;
        let slot_str = slot_name.to_string();
        let build_fn = format_ident!("__build_{}", slot_name);
        let sheet_expr = stylesheet::emit_sheet_expr(&slot.sheet);
        build_fns.push(quote! {
            #[doc(hidden)]
            fn #build_fn() -> ::runtime_core::StyleSheet {
                let _ = ::core::marker::PhantomData::<#theme_ty>;
                #sheet_expr
            }
        });
        let premintable = !stylesheet::sheet_has_dynamic_font(&slot.sheet);
        let class = if premintable {
            // Per-slot class base: the declaration's source hash mixed with
            // the slot name, so slots never share a class and any edit to
            // the declaration moves every class it mints.
            let h = stylesheet::content_hash(&format!("{content_hash:x}#{slot_str}"));
            let c = format!("iy-{:012x}", h & 0xffff_ffff_ffff);
            quote! { ::core::option::Option::Some(#c) }
        } else {
            quote! { ::core::option::Option::None }
        };
        let extend = match &slot.extend {
            Some(p) => quote! {
                ::core::option::Option::Some(#p as fn(&mut ::runtime_core::SlotRestyle<'_, #styles>))
            },
            None => quote! { ::core::option::Option::None },
        };
        slot_defs.push(quote! {
            ::runtime_core::SlotDef {
                name: #slot_str,
                build: #styles::#build_fn,
                class: #class,
                extend: #extend,
            }
        });
        let slot_docs = &slot.docs;
        let acc_doc = format!("The `{slot_str}` slot, ready to style an element.");
        accessors.push(quote! {
            #(#slot_docs)*
            #[doc = #acc_doc]
            pub fn #slot_name() -> ::runtime_core::SlotStyle<#styles> {
                ::runtime_core::SlotStyle::new(#i)
            }
        });
        let rs_doc = format!("Restyle the `{slot_str}` slot.");
        restyle_methods.push(quote! {
            #[doc = #rs_doc]
            pub fn #slot_name(&mut self) -> ::runtime_core::SlotRestyle<'_, #styles> {
                self.0.slot(#i)
            }
        });
        let ov_doc = format!("Per-instance override for the `{slot_str}` slot.");
        override_fields.push(quote! {
            #[doc = #ov_doc]
            pub #slot_name: ::core::option::Option<::runtime_core::SlotOverride>
        });
        let set_doc = format!("Set the `{slot_str}` slot's override (a `StyleRules`, or a sheet resolved at its defaults).");
        override_setters.push(quote! {
            #[doc = #set_doc]
            pub fn #slot_name(mut self, o: impl ::core::convert::Into<::runtime_core::SlotOverride>) -> Self {
                self.#slot_name = ::core::option::Option::Some(o.into());
                self
            }
        });
    }
    let n_slots = slots.len();

    // ---- axis types ---------------------------------------------------------
    let mut axis_items = Vec::new();
    let mut axis_defs = Vec::new();
    for axis in &axes {
        let axis_str = axis.name.to_string();
        let ty = format_ident!("{}{}", name, stylesheet::pascal(&axis.name));
        let values_str: Vec<String> = axis.values.iter().map(|(_, v)| v.to_string()).collect();
        let default_str = axis.default.as_ref().map(|d| d.to_string());
        let default_tok = match &default_str {
            Some(d) => quote! { ::core::option::Option::Some(#d) },
            None => quote! { ::core::option::Option::None },
        };
        axis_defs.push(quote! {
            ::runtime_core::AxisDef { name: #axis_str, values: &[#(#values_str),*], default: #default_tok }
        });
        let consts = axis.values.iter().map(|(vdocs, v)| {
            let c = stylesheet::pascal(v);
            let s = v.to_string();
            let doc = format!("The built-in `{s}` value of `{axis_str}`.");
            quote! {
                #(#vdocs)*
                #[doc = #doc]
                pub const #c: Self = Self(#s);
            }
        });
        let builtin_list = axis.values.iter().map(|(_, v)| {
            let c = stylesheet::pascal(v);
            quote! { Self::#c }
        });
        let axis_docs = &axis.docs;
        let ty_doc = format!(
            "The `{axis_str}` axis of [`{styles_name}`] — an open set: the associated consts are the \
             built-in values, [`{ty}::define`] adds one, and a restyle arm gives it a look."
        );
        let default_impl = axis.default.as_ref().map(|d| {
            let c = stylesheet::pascal(d);
            quote! {
                /// The app's `set_default` choice, else the declared default.
                impl ::core::default::Default for #ty {
                    fn default() -> Self {
                        match ::runtime_core::component_styles::axis_default::<#styles>(#axis_str) {
                            ::core::option::Option::Some(v) => Self(v),
                            ::core::option::Option::None => Self::#c,
                        }
                    }
                }
            }
        });
        let is_bool = values_str.len() == 2
            && values_str.iter().any(|v| v == "on")
            && values_str.iter().any(|v| v == "off");
        let bool_impl = is_bool.then(|| {
            quote! {
                /// `true` → `on`, `false` → `off`.
                impl ::core::convert::From<bool> for #ty {
                    fn from(on: bool) -> Self {
                        if on { Self("on") } else { Self("off") }
                    }
                }
            }
        });
        let remote_keys = axis.values.iter().map(|(_, v)| {
            let c = stylesheet::pascal(v);
            quote! { ::runtime_vocabulary::__remote_key!(#ty, |v| v.0, #ty::#c); }
        });
        #[cfg(feature = "catalog")]
        let catalog = {
            let ty_str = ty.to_string();
            let entries = axis.values.iter().map(|(_, v)| {
                let short = stylesheet::pascal(v).to_string();
                let doc = format!("The built-in `{v}` value of `{ty_str}`.");
                let via = format!("{ty_str}");
                quote! {
                    ::runtime_core::__mcp::inventory::submit! {
                        ::runtime_core::__mcp::ValueEntry {
                            short_name: #short,
                            module_path: module_path!(),
                            docs: #doc,
                            value_of: #ty_str,
                            via: #via,
                        }
                    }
                }
            });
            quote! { #(#entries)* }
        };
        #[cfg(not(feature = "catalog"))]
        let catalog = TokenStream2::new();
        axis_items.push(quote! {
            #(#axis_docs)*
            #[doc = #ty_doc]
            #[derive(::core::clone::Clone, ::core::marker::Copy, ::core::cmp::PartialEq, ::core::cmp::Eq, ::core::hash::Hash, ::core::fmt::Debug)]
            #vis struct #ty(&'static str);

            #[allow(non_upper_case_globals)]
            impl #ty {
                #(#consts)*

                /// Every built-in value, in declaration order.
                pub const BUILTINS: &'static [Self] = &[#(#builtin_list),*];

                /// An app-defined value of this axis. Give it a look with a
                /// restyle arm (`s.<slot>().arm(VALUE, |t| …)`); a value no
                /// slot styles warns in debug builds when used. `key` becomes
                /// part of a class name: lowercase ASCII letters, digits and
                /// `_` only (checked at compile time in a `const`).
                pub const fn define(key: &'static str) -> Self {
                    let b = key.as_bytes();
                    assert!(!b.is_empty(), "an axis value key is not empty");
                    let mut i = 0;
                    while i < b.len() {
                        let c = b[i];
                        assert!(
                            (c >= b'a' && c <= b'z') || (c >= b'0' && c <= b'9') || c == b'_',
                            "an axis value key is lowercase ASCII letters, digits and `_`"
                        );
                        i += 1;
                    }
                    Self(key)
                }

                /// The value's key — the arm name in the slot sheets.
                pub const fn key(self) -> &'static str {
                    self.0
                }
            }

            #default_impl
            #bool_impl

            impl ::runtime_core::VariantEnum for #ty {
                fn as_variant_str(self) -> &'static str {
                    self.0
                }
                fn all_variants() -> &'static [Self] {
                    Self::BUILTINS
                }
            }

            impl ::runtime_core::AxisKey for #ty {
                type Styles = #styles;
                const AXIS: &'static str = #axis_str;
            }

            ::runtime_vocabulary::__remote_keyed!(#ty, |v| v.0);
            #(#remote_keys)*
            #catalog
        });
    }

    let styles_doc = format!(
        "The slot styles of `{name}`: the registry key its restyles, axis defaults and composed \
         sheets hang off. See `runtime_core::component_styles`."
    );
    let restyle_doc = format!("What a [`{styles_name}::restyle`] closure receives: one accessor per slot.");
    let overrides_doc = format!(
        "Per-instance slot overrides for `{name}` — one optional override per slot, layered over \
         that slot's style on one element."
    );

    quote! {
        #(#docs)*
        #[doc = #styles_doc]
        #vis struct #styles;

        impl #styles {
            #(#build_fns)*
            #(#accessors)*

            /// A shared axis-selection set for this component's slots.
            pub fn values() -> ::runtime_core::AxisValues<#styles> {
                ::runtime_core::AxisValues::new()
            }

            /// Restyle this component app-wide. `f` runs at every
            /// composition (so it is `Fn`); register it at boot. A second
            /// call from the same call site replaces the first.
            #[track_caller]
            pub fn restyle(f: impl ::core::ops::Fn(&mut #restyle_ty<'_, '_>) + 'static) {
                ::runtime_core::component_styles::restyle::<#styles>(move |r| f(&mut #restyle_ty(r)))
            }

            /// Make `value` what an unset axis means, app-wide.
            pub fn set_default<A: ::runtime_core::AxisKey<Styles = #styles>>(value: A) {
                ::runtime_core::component_styles::set_axis_default(value)
            }
        }

        impl ::runtime_core::ComponentStyles for #styles {
            type Tokens = <#theme_ty as ::runtime_core::TokenVocabulary>::Tokens;
            type Overrides = #overrides_ty;
            const NAME: &'static str = #styles_name;
            fn slots() -> &'static [::runtime_core::SlotDef<Self>] {
                static SLOTS: [::runtime_core::SlotDef<#styles>; #n_slots] = [#(#slot_defs),*];
                &SLOTS
            }
            fn axes() -> &'static [::runtime_core::AxisDef] {
                static AXES: &[::runtime_core::AxisDef] = &[#(#axis_defs),*];
                AXES
            }
        }

        #[doc = #restyle_doc]
        #vis struct #restyle_ty<'r, 'c>(&'r mut ::runtime_core::Restyle<'c, #styles>);

        impl<'r, 'c> #restyle_ty<'r, 'c> {
            #(#restyle_methods)*
        }

        #[doc = #overrides_doc]
        #[derive(::core::clone::Clone, ::core::default::Default)]
        #vis struct #overrides_ty {
            #(#override_fields,)*
        }

        impl #overrides_ty {
            #(#override_setters)*
        }

        #(#axis_items)*

        // The premint dump composes every linked component after building
        // the app (see `runtime_core::premint::PREMINT_COMPONENT_STYLES`).
        // The `#[cfg]` sits on the inner static so the `unexpected_cfgs`
        // allow on this const covers it.
        #[allow(unexpected_cfgs)]
        const _: () = {
            #[cfg(idealyst_premint_dump)]
            #[::runtime_core::premint::linkme::distributed_slice(
                ::runtime_core::premint::PREMINT_COMPONENT_STYLES
            )]
            #[linkme(crate = ::runtime_core::premint::linkme)]
            static __PREMINT_COMPONENT: fn() =
                ::runtime_core::component_styles::__compose_for_dump::<#styles>;
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(input: TokenStream2) -> String {
        let decl: ComponentStylesDecl = syn::parse2(input).expect("parse component_styles");
        let out = emit(decl, 0);
        syn::parse2::<syn::File>(out.clone()).expect("expansion is a list of items");
        out.to_string()
    }

    fn parse_err(input: TokenStream2) -> String {
        match syn::parse2::<ComponentStylesDecl>(input) {
            Ok(decl) => emit(decl, 0).to_string(),
            Err(e) => e.to_string(),
        }
    }

    #[test]
    fn generates_the_styles_type_axis_types_restyle_handle_and_overrides() {
        let out = expand(quote! {
            pub Field<()> {
                axis appearance { #[default] contained, outlined }
                slot input {
                    base(_t) { padding: 8 }
                    variant size { sm(_t) { height: 32 } #[default] md(_t) { height: 40 } }
                }
                slot label { base(_t) { padding: 2 } }
            }
        });
        for needle in [
            "pub struct FieldStyles",
            "pub struct FieldAppearance",
            "pub struct FieldSize",
            "pub const Contained : Self = Self (\"contained\")",
            "pub const Md : Self = Self (\"md\")",
            "pub struct FieldRestyle",
            "pub struct FieldSlotStyles",
            "pub input : :: core :: option :: Option < :: runtime_core :: SlotOverride >",
            "pub fn input () -> :: runtime_core :: SlotStyle < FieldStyles >",
            "pub fn label () -> :: runtime_core :: SlotStyle < FieldStyles >",
            "PREMINT_COMPONENT_STYLES",
            "type Styles = FieldStyles",
        ] {
            assert!(out.contains(needle), "missing `{needle}` in:\n{out}");
        }
    }

    #[test]
    fn an_on_off_axis_converts_from_bool() {
        let out = expand(quote! {
            pub Seg<()> { slot item { base(_t) {} variant selected { on(_t) {} #[default] off(_t) {} } } }
        });
        assert!(out.contains("impl :: core :: convert :: From < bool > for SegSelected"), "{out}");
    }

    #[test]
    fn slots_get_distinct_classes() {
        let out = expand(quote! {
            pub Two<()> { slot a { base(_t) {} } slot b { base(_t) {} } }
        });
        let classes: Vec<&str> = out.match_indices("\"iy-").map(|(i, _)| &out[i..i + 17]).collect();
        assert_eq!(classes.len(), 2, "{out}");
        assert_ne!(classes[0], classes[1]);
    }

    #[test]
    fn extend_hook_lands_on_its_slot() {
        let out = expand(quote! {
            pub Btn<()> { slot root { extend(crate::tones::extend) base(_t) {} } }
        });
        assert!(out.contains("crate :: tones :: extend as fn"), "{out}");
    }

    #[test]
    fn conflicting_defaults_across_slots_are_rejected() {
        let err = parse_err(quote! {
            pub X<()> {
                slot a { base(_t) {} variant size { #[default] sm(_t) {} md(_t) {} } }
                slot b { base(_t) {} variant size { sm(_t) {} #[default] md(_t) {} } }
            }
        });
        assert!(err.contains("one axis has one default"), "{err}");
    }

    #[test]
    fn overrides_and_reserved_names_are_rejected() {
        let err = parse_err(quote! { pub X<()> { slot a { base(_t) {} override padding: f32 } } });
        assert!(err.contains("per-instance overrides"), "{err}");
        let err = parse_err(quote! { pub X<()> { slot restyle { base(_t) {} } } });
        assert!(err.contains("generated method"), "{err}");
        let err = parse_err(quote! { pub X<()> { axis a { b } } });
        assert!(err.contains("at least one `slot`"), "{err}");
    }
}
