//! `axis_values! { … }` — app-defined values of a `component_styles!` axis,
//! named like the built-ins.
//!
//! A built-in value is an associated const whose name is the PascalCase of
//! its key (`soft` → `ChipLook::Soft`). This macro runs the same mapping
//! backwards, so an app value is written once, as a name, and its key is
//! derived: `Glow` → `"glow"`, `PillShape` → `"pill_shape"`. Two forms:
//!
//! ```ignore
//! // In the crate that declares the axis type: inherent consts.
//! axis_values! { impl ChipLook { Glow, Dashed } }
//!
//! // From any other crate: a trait the type implements. Rust allows no
//! // inherent impl on a foreign type; a trait's associated const reads
//! // the same at call sites (`ButtonVariant::Raised`) with the trait in
//! // scope, and is usable in patterns.
//! axis_values! { pub trait BrandButtonVariants for ButtonVariant { Raised, Tinted } }
//! ```
//!
//! Each value expands to `<Type>::define("<key>")`, so the key is still
//! checked at compile time by `define`'s const assertions.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{braced, Attribute, Ident, Path, Token, Visibility};

/// One value: its doc/attributes and its PascalCase name.
pub(crate) struct AxisValue {
    attrs: Vec<Attribute>,
    name: Ident,
}

impl Parse for AxisValue {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(Self { attrs: input.call(Attribute::parse_outer)?, name: input.parse()? })
    }
}

enum Form {
    /// `impl Type { … }` — inherent consts, same crate as the type.
    Inherent,
    /// `<vis> trait Name for Type { … }` — a trait carrying the consts.
    Trait { vis: Visibility, name: Ident },
}

pub(crate) struct AxisValuesDecl {
    attrs: Vec<Attribute>,
    form: Form,
    ty: Path,
    values: Vec<AxisValue>,
}

impl Parse for AxisValuesDecl {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        let form = if input.peek(Token![impl]) {
            input.parse::<Token![impl]>()?;
            Form::Inherent
        } else {
            let vis: Visibility = input.parse()?;
            input.parse::<Token![trait]>().map_err(|e| {
                syn::Error::new(
                    e.span(),
                    "expected `impl Type { Name, … }` (in the axis type's own crate) or \
                     `pub trait TraitName for Type { Name, … }` (from any crate)",
                )
            })?;
            let name: Ident = input.parse()?;
            input.parse::<Token![for]>()?;
            Form::Trait { vis, name }
        };
        let ty: Path = input.parse()?;
        let body;
        braced!(body in input);
        let values: Punctuated<AxisValue, Token![,]> = body.parse_terminated(AxisValue::parse, Token![,])?;
        let values: Vec<AxisValue> = values.into_iter().collect();
        if values.is_empty() {
            return Err(syn::Error::new(ty_span(&ty), "list at least one value"));
        }
        for v in &values {
            key_of(&v.name)?;
        }
        Ok(Self { attrs, form, ty, values })
    }
}

fn ty_span(ty: &Path) -> proc_macro2::Span {
    ty.segments.last().map_or_else(proc_macro2::Span::call_site, |s| s.ident.span())
}

/// The key a value's name stands for: the inverse of the PascalCase a
/// built-in const is named with. `Glow` → `glow`, `PillShape` →
/// `pill_shape`, `Size2x` → `size2x`. Names must be PascalCase ASCII, so
/// the round trip through `pascal` gives the name back.
pub(crate) fn key_of(name: &Ident) -> syn::Result<String> {
    let s = name.to_string();
    let s = s.strip_prefix("r#").unwrap_or(&s);
    let err = || {
        syn::Error::new(
            name.span(),
            format!(
                "`{s}`: an axis value is named in PascalCase ASCII (`Glow`, `PillShape`) — \
                 its key is derived from the name (`glow`, `pill_shape`)"
            ),
        )
    };
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_ascii_uppercase() => {}
        _ => return Err(err()),
    }
    if !s.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(err());
    }
    let mut key = String::with_capacity(s.len() + 4);
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                key.push('_');
            }
            key.push(c.to_ascii_lowercase());
        } else {
            key.push(c);
        }
    }
    Ok(key)
}

pub(crate) fn emit(decl: AxisValuesDecl) -> TokenStream2 {
    let AxisValuesDecl { attrs, form, ty, values } = decl;
    let ty_str = quote!(#ty).to_string().replace(' ', "");
    let mut decls = Vec::new();
    let mut defs = Vec::new();
    for v in &values {
        let name = &v.name;
        let vattrs = &v.attrs;
        // Validated in `parse`.
        let key = key_of(name).expect("checked when parsed");
        let doc = format!("App-defined `{key}` value of `{ty_str}`, from `axis_values!`.");
        decls.push(quote! {
            #(#vattrs)*
            #[doc = #doc]
            const #name: #ty;
        });
        defs.push(quote! {
            #(#vattrs)*
            #[doc = #doc]
            pub const #name: #ty = <#ty>::define(#key);
        });
    }
    match form {
        Form::Inherent => quote! {
            #(#attrs)*
            #[allow(non_upper_case_globals)]
            impl #ty {
                #(#defs)*
            }
        },
        Form::Trait { vis, name } => {
            let impls = values.iter().map(|v| {
                let n = &v.name;
                let key = key_of(n).expect("checked when parsed");
                quote! { const #n: #ty = <#ty>::define(#key); }
            });
            quote! {
                #(#attrs)*
                #[allow(non_upper_case_globals)]
                #vis trait #name {
                    #(#decls)*
                }
                #[allow(non_upper_case_globals)]
                impl #name for #ty {
                    #(#impls)*
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proc_macro2::Span;

    fn key(s: &str) -> Result<String, String> {
        key_of(&Ident::new(s, Span::call_site())).map_err(|e| e.to_string())
    }

    #[test]
    fn the_key_is_the_name_in_snake_case() {
        assert_eq!(key("Glow").unwrap(), "glow");
        assert_eq!(key("PillShape").unwrap(), "pill_shape");
        assert_eq!(key("Size2x").unwrap(), "size2x");
        assert_eq!(key("A").unwrap(), "a");
    }

    /// The derived key round-trips through the PascalCase a built-in const
    /// is named with, so `define`d and declared values share one naming.
    #[test]
    fn the_key_round_trips_through_the_builtin_naming() {
        for name in ["Glow", "PillShape", "Size2x", "Raised"] {
            let k = key(name).unwrap();
            let back = crate::stylesheet::pascal(&Ident::new(&k, Span::call_site()));
            assert_eq!(back.to_string(), name);
        }
    }

    #[test]
    fn a_name_that_is_not_pascal_case_is_refused() {
        for bad in ["glow", "Pill_Shape", "_Glow"] {
            assert!(key(bad).unwrap_err().contains("PascalCase"), "{bad}");
        }
    }

    #[test]
    fn both_forms_parse_and_emit() {
        let inherent: AxisValuesDecl = syn::parse_quote! { impl ChipLook { /// Doc.
            Glow, Dashed } };
        let out = emit(inherent).to_string();
        assert!(out.contains("impl ChipLook"));
        assert!(out.contains("pub const Glow : ChipLook = < ChipLook > :: define (\"glow\")"), "{out}");

        let tr: AxisValuesDecl = syn::parse_quote! { pub trait Brand for idea_ui::ButtonVariant { Raised } };
        let out = emit(tr).to_string();
        assert!(out.contains("pub trait Brand"), "{out}");
        assert!(out.contains("impl Brand for idea_ui :: ButtonVariant"), "{out}");
        assert!(out.contains("define (\"raised\")"), "{out}");
    }
}
