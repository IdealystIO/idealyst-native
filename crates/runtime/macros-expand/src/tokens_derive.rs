//! `#[derive(Tokens)]`: an ordinary struct becomes a token set.
//!
//! Every field is a token (a `TokenLeaf` — `Color`, `Length`, `f32`, or an
//! author newtype) or a namespace (another `#[derive(Tokens)]` struct). The
//! derive cannot tell which from the field's type — a proc macro sees only
//! the type's spelling — so it emits the same code for both and lets the
//! `TokenField` trait decide: the blanket impl over `TokenLeaf` answers for
//! leaves, the nested derive's own impl for namespaces. For the same
//! reason each field gets BOTH an accessor method and an accessor field of
//! the same name: a leaf is read as `t.surface()`, a namespace is walked as
//! `t.accent.soft_bg()` (`t.accent()` works too). A leaf's field holds a
//! `LeafSlot`, which has no methods.
//!
//! Emitted for `struct AppTheme { surface: Color, accent: Swatch }`:
//!
//! - `struct AppThemeTokens<R>` — the accessor namespace, generic over the
//!   theme ROOT `R` whose name table its methods index (see
//!   `runtime_vocabulary::tokens` for why names are resolved at the root);
//! - `impl TokenField for AppTheme` — the field tree as `const` data, the
//!   leaf count, and the accessor/value plumbing;
//! - `impl Tokens for AppTheme` — `.with`, and the install family.
//!
//! With `#[tokens(default = path)]` the struct is a THEME, and also gets
//! `impl TokenTheme` (the const-evaluated name and path tables, the
//! default instance), `impl TokenVocabulary` (so `stylesheet! { X<AppTheme> }`
//! binds `AppThemeTokens<AppTheme>`), a forced evaluation of the name
//! table (a collision is a compile error at the definition, not at first
//! use), and the catalog registration.
//!
//! Attributes:
//!
//! - struct: `#[tokens(default = path::to::fn)]` (makes it a theme),
//!   `#[tokens(prefix = "app")]` (prefixes every token name; theme only);
//! - field: `#[tokens(skip)]` (not a token — kept in the struct, absent
//!   from the vocabulary), `#[tokens(app_background)]` (this `Color` leaf
//!   themes the host surface; one per theme, on the theme's own fields).

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, LitStr};

struct StructAttrs {
    default: Option<syn::Path>,
    prefix: Option<LitStr>,
}

#[derive(Default)]
struct FieldAttrs {
    skip: bool,
    app_background: bool,
}

fn struct_attrs(input: &DeriveInput) -> syn::Result<StructAttrs> {
    let mut out = StructAttrs { default: None, prefix: None };
    for attr in input.attrs.iter().filter(|a| a.path().is_ident("tokens")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("default") {
                out.default = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("prefix") {
                let lit: LitStr = meta.value()?.parse()?;
                let v = lit.value();
                if v.is_empty() || !v.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') {
                    return Err(syn::Error::new(lit.span(), "a token prefix is lowercase kebab case (`app`, `brand-ui`)"));
                }
                out.prefix = Some(lit);
                Ok(())
            } else {
                Err(meta.error("unknown `tokens` attribute on a struct; expected `default = path::to::fn` or `prefix = \"…\"`"))
            }
        })?;
    }
    if out.prefix.is_some() && out.default.is_none() {
        return Err(syn::Error::new(
            out.prefix.as_ref().unwrap().span(),
            "`prefix` applies to a theme's token names; add `default = path::to::fn` to make this struct a theme",
        ));
    }
    Ok(out)
}

fn field_attrs(field: &syn::Field) -> syn::Result<FieldAttrs> {
    let mut out = FieldAttrs::default();
    for attr in field.attrs.iter().filter(|a| a.path().is_ident("tokens")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("skip") {
                out.skip = true;
                Ok(())
            } else if meta.path.is_ident("app_background") {
                out.app_background = true;
                Ok(())
            } else {
                Err(meta.error("unknown `tokens` attribute on a field; expected `skip` or `app_background`"))
            }
        })?;
    }
    if out.skip && out.app_background {
        return Err(syn::Error::new(field.span(), "a `skip`ped field is not a token, so it cannot be the `app_background`"));
    }
    Ok(out)
}

/// `soft_bg` → `soft-bg`; a raw identifier loses its `r#`.
fn segments(ident: &syn::Ident) -> (String, String) {
    let path = ident.to_string().trim_start_matches("r#").to_string();
    (path.replace('_', "-"), path)
}

pub(crate) fn derive(input: DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let vis = &input.vis;
    if !input.generics.params.is_empty() || input.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            input.generics.span(),
            "#[derive(Tokens)] takes no generics: a token set's names are fixed at compile time",
        ));
    }
    let fields = match &input.data {
        Data::Struct(s) => match &s.fields {
            Fields::Named(f) => &f.named,
            Fields::Unit => {
                return Err(syn::Error::new(name.span(), "#[derive(Tokens)] needs named fields — each field is a token"))
            }
            Fields::Unnamed(f) => {
                return Err(syn::Error::new(f.span(), "#[derive(Tokens)] needs named fields — a token's name is its field name"))
            }
        },
        Data::Enum(e) => {
            return Err(syn::Error::new(
                e.enum_token.span,
                "#[derive(Tokens)] needs a struct: a theme assigns a value to EVERY token, an enum holds one variant",
            ))
        }
        Data::Union(u) => return Err(syn::Error::new(u.union_token.span, "#[derive(Tokens)] needs a struct")),
    };
    let attrs = struct_attrs(&input)?;

    let rv = quote!(::runtime_vocabulary::tokens);
    let accessor = format_ident!("{}Tokens", name);

    // The token fields, in declaration order, with each one's offset into
    // this set's leaves: the sum of the preceding fields' leaf counts.
    struct TokenFieldInfo<'a> {
        field: &'a syn::Field,
        ident: &'a syn::Ident,
        offset: TokenStream2,
        docs: Vec<&'a syn::Attribute>,
        app_background: bool,
    }
    let mut infos: Vec<TokenFieldInfo> = Vec::new();
    let mut running = quote!(0usize);
    for field in fields {
        let fa = field_attrs(field)?;
        if fa.skip {
            continue;
        }
        let ident = field.ident.as_ref().expect("named field");
        let ty = &field.ty;
        let docs = field.attrs.iter().filter(|a| a.path().is_ident("doc")).collect();
        infos.push(TokenFieldInfo { field, ident, offset: running.clone(), docs, app_background: fa.app_background });
        running = quote!(#running + <#ty as #rv::TokenField>::LEAF_COUNT);
    }
    let leaf_count = running;

    let backgrounds: Vec<&TokenFieldInfo> = infos.iter().filter(|i| i.app_background).collect();
    if backgrounds.len() > 1 {
        return Err(syn::Error::new(backgrounds[1].field.span(), "only one field can be the `app_background`"));
    }
    if let (Some(bg), None) = (backgrounds.first(), &attrs.default) {
        return Err(syn::Error::new(
            bg.field.span(),
            "`app_background` belongs on a field of the theme root — the struct with `#[tokens(default = …)]`",
        ));
    }

    let nodes = infos.iter().map(|i| {
        let ty = &i.field.ty;
        let (kebab, path) = segments(i.ident);
        quote_spanned! {ty.span()=>
            #rv::TokenNode {
                name: #kebab,
                path: #path,
                kind: <#ty as #rv::TokenField>::KIND,
                children: <#ty as #rv::TokenField>::NODES,
            }
        }
    });

    let slot_fields = infos.iter().map(|i| {
        let (ident, ty, fvis, docs) = (i.ident, &i.field.ty, &i.field.vis, &i.docs);
        quote! { #(#docs)* #fvis #ident: <#ty as #rv::TokenField>::Slot<R>, }
    });
    let slot_inits = infos.iter().map(|i| {
        let (ident, ty, offset) = (i.ident, &i.field.ty, &i.offset);
        quote! { #ident: <#ty as #rv::TokenField>::slot::<R>(index + #offset), }
    });
    let methods = infos.iter().map(|i| {
        let (ident, ty, fvis, docs, offset) = (i.ident, &i.field.ty, &i.field.vis, &i.docs, &i.offset);
        quote! {
            #(#docs)*
            #[inline]
            #fvis fn #ident(&self) -> <#ty as #rv::TokenField>::Read<R> {
                <#ty as #rv::TokenField>::read::<R>(self.__index + #offset)
            }
        }
    });
    let collects = infos.iter().map(|i| {
        let ident = i.ident;
        quote! { #rv::TokenField::collect_values(&self.#ident, out); }
    });
    // A field type that is neither a leaf nor a set fails here, spanned on
    // the field, with `TokenField`'s explanation.
    let field_checks = infos.iter().map(|i| {
        let ty = &i.field.ty;
        quote_spanned! {ty.span()=> __is_token_field::<#ty>(); }
    });

    let accessor_doc = format!(
        "The token accessors of [`{name}`]: what a `stylesheet!` block binding is for a sheet \
         declared over a theme containing it. A leaf is read by calling it (`t.surface()`); a \
         nested set is reached through its field (`t.accent.soft_bg()`)."
    );

    let mut out = quote! {
        #[doc = #accessor_doc]
        #vis struct #accessor<R: #rv::TokenTheme> {
            #[doc(hidden)]
            __index: usize,
            #[doc(hidden)]
            __root: ::core::marker::PhantomData<fn() -> R>,
            #(#slot_fields)*
        }

        impl<R: #rv::TokenTheme> #accessor<R> {
            #(#methods)*
        }

        impl<R: #rv::TokenTheme> ::core::clone::Clone for #accessor<R> {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl<R: #rv::TokenTheme> ::core::marker::Copy for #accessor<R> {}

        impl<R: #rv::TokenTheme> ::core::default::Default for #accessor<R> {
            /// The accessors of a theme ROOT (its tokens start at index 0).
            fn default() -> Self {
                <#name as #rv::TokenField>::slot::<R>(0)
            }
        }

        impl #rv::TokenField for #name {
            const LEAF_COUNT: usize = #leaf_count;
            const KIND: ::core::option::Option<&'static str> = ::core::option::Option::None;
            const NODES: &'static [#rv::TokenNode] = &[#(#nodes),*];
            type Read<R: #rv::TokenTheme> = #accessor<R>;
            type Slot<R: #rv::TokenTheme> = #accessor<R>;

            fn read<R: #rv::TokenTheme>(index: usize) -> #accessor<R> {
                <Self as #rv::TokenField>::slot::<R>(index)
            }

            fn slot<R: #rv::TokenTheme>(index: usize) -> #accessor<R> {
                #accessor {
                    __index: index,
                    __root: ::core::marker::PhantomData,
                    #(#slot_inits)*
                }
            }

            fn collect_values(&self, out: &mut ::std::vec::Vec<::runtime_vocabulary::glue::TokenValue>) {
                #(#collects)*
            }
        }

        impl #rv::Tokens for #name {}

        const _: () = {
            fn __is_token_field<T: #rv::TokenField + ?Sized>() {}
            #[allow(dead_code)]
            fn __check() {
                #(#field_checks)*
            }
        };
    };

    if let Some(default) = &attrs.default {
        let prefix = attrs.prefix.as_ref().map(LitStr::value).unwrap_or_default();
        let type_name = name.to_string();
        let app_background = match backgrounds.first() {
            Some(bg) => {
                let offset = &bg.offset;
                quote!(::core::option::Option::Some(#offset))
            }
            None => quote!(::core::option::Option::None),
        };
        let background_check = backgrounds.first().map(|bg| {
            let ty = &bg.field.ty;
            quote_spanned! {ty.span()=>
                const _: () = {
                    fn __is_color_leaf<T: #rv::TokenLeaf<Value = ::runtime_vocabulary::glue::Color>>() {}
                    #[allow(dead_code)]
                    fn __check() { __is_color_leaf::<#ty>(); }
                };
            }
        });
        let table = |path: bool| {
            quote! {{
                const NODES: &[#rv::TokenNode] = <#name as #rv::TokenField>::NODES;
                const LEN: usize = #rv::__table_len(NODES, #prefix, #path);
                const COUNT: usize = <#name as #rv::TokenField>::LEAF_COUNT;
                const TABLE: ([u8; LEN], [usize; COUNT]) = #rv::__table(NODES, #prefix, #path);
                const STRS: [&str; COUNT] = #rv::__split(&TABLE.0, &TABLE.1);
                &STRS
            }}
        };
        let names = table(false);
        let paths = table(true);
        out.extend(quote_spanned! {Span::call_site()=>
            impl #rv::TokenTheme for #name {
                const NAMES: &'static [&'static str] = #names;
                const PATHS: &'static [&'static str] = #paths;
                const APP_BACKGROUND: ::core::option::Option<usize> = #app_background;
                const NAME: &'static str = #type_name;
                fn default_theme() -> Self {
                    #default()
                }
            }

            impl ::runtime_vocabulary::glue::TokenVocabulary for #name {
                type Tokens = #accessor<#name>;
            }

            // Evaluated here so a name collision fails at the theme's
            // definition rather than at its first use.
            const _: () = #rv::__assert_unique(<#name as #rv::TokenTheme>::NAMES);

            #background_check

            ::runtime_vocabulary::register_style_token_set!(#name);
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expand(src: &str) -> Result<String, String> {
        let input: DeriveInput = syn::parse_str(src).unwrap();
        derive(input).map(|t| t.to_string()).map_err(|e| e.to_string())
    }

    #[test]
    fn a_field_yields_a_kebab_name_segment_and_its_accessor_path() {
        let out = expand("struct Swatch { soft_bg: Color, r#type: Color }").unwrap();
        assert!(out.contains("name : \"soft-bg\""), "{out}");
        assert!(out.contains("path : \"soft_bg\""), "{out}");
        assert!(out.contains("name : \"type\""), "raw identifiers drop `r#`: {out}");
        assert!(out.contains("fn soft_bg (& self)"), "{out}");
    }

    /// A field's doc comment documents both its accessor method and its
    /// accessor field.
    #[test]
    fn field_docs_carry_onto_the_accessors() {
        let out = expand("struct Swatch { /// Tinted background.\n soft_bg: Color }").unwrap();
        assert_eq!(out.matches("doc = \" Tinted background.\"").count(), 2, "{out}");
    }

    #[test]
    fn a_shape_without_default_is_not_a_theme() {
        let out = expand("struct Swatch { a: Color }").unwrap();
        assert!(!out.contains("TokenTheme for Swatch"), "{out}");
        assert!(!out.contains("TokenVocabulary"), "{out}");
        assert!(!out.contains("register_style_token_set"), "{out}");
    }

    #[test]
    fn a_theme_gets_names_vocabulary_and_catalog() {
        let out = expand("#[tokens(default = light, prefix = \"app\")] struct App { a: Color }").unwrap();
        assert!(out.contains("TokenTheme for App"), "{out}");
        assert!(out.contains("TokenVocabulary for App"), "{out}");
        assert!(out.contains("register_style_token_set ! (App)"), "{out}");
        assert!(out.contains("\"app\""), "the prefix reaches the name table: {out}");
        assert!(out.contains("light ()"), "the default fn is called: {out}");
    }

    #[test]
    fn skipped_fields_have_no_accessor() {
        let out = expand("struct T { a: Color, #[tokens(skip)] font: FontFamily }").unwrap();
        assert!(!out.contains("fn font"), "{out}");
        assert!(!out.contains("FontFamily as"), "{out}");
    }

    #[test]
    fn rejects_what_cannot_be_a_token_set() {
        for (src, msg) in [
            ("enum E { A }", "needs a struct"),
            ("struct T(Color);", "named fields"),
            ("struct T;", "named fields"),
            ("struct T<C> { a: C }", "no generics"),
            ("#[tokens(prefix = \"x\")] struct T { a: Color }", "add `default"),
            ("#[tokens(default = f, prefix = \"Bad_Prefix\")] struct T { a: Color }", "kebab"),
            ("#[tokens(bogus)] struct T { a: Color }", "unknown `tokens` attribute"),
            ("struct T { #[tokens(app_background)] a: Color }", "theme root"),
            (
                "#[tokens(default = f)] struct T { #[tokens(app_background)] a: Color, #[tokens(app_background)] b: Color }",
                "only one field",
            ),
        ] {
            let err = expand(src).expect_err(src);
            assert!(err.contains(msg), "`{src}` → {err}");
        }
    }
}
