//! `#[derive(Tokens)]`: an ordinary struct becomes a token set.
//!
//! Every field is a token (a `TokenLeaf` — `Color`, `Length`, `f32`, a
//! `Tokenized<_>`, or an author newtype) or a namespace (another
//! `#[derive(Tokens)]` struct). The derive cannot tell which from the
//! field's type — a proc macro sees only the type's spelling — so it emits
//! the same code for both and lets the `TokenField` trait decide: the
//! blanket impl over `TokenLeaf` answers for leaves, the nested derive's
//! own impl for namespaces. For the same reason each field gets BOTH an
//! accessor method and an accessor field of the same name: a leaf is read
//! as `t.surface()`, a namespace is walked as `t.accent.soft_bg()`
//! (`t.accent()` works too). A leaf's field holds a `LeafSlot`, which has
//! no methods.
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
//! Struct attributes:
//!
//! - `default = path::to::fn` — makes it a theme (fallbacks come from it);
//! - `prefix = "app"` — prefixes every token name (theme only);
//! - `vocabulary = "Name"` — the name the MCP catalog lists the vocabulary
//!   under, when sheets name it through another type (idea-theme's sheets
//!   write `<IdeaThemeRef>`; theme only, defaults to the struct's name);
//! - `as = Kind` — every field installs as `Kind` (see the field form);
//! - `computed(name: Type = path::to::fn, …)` — a token with no stored
//!   field: its value is `fn(&Self) -> Type` (`Type` a `TokenLeaf`),
//!   appended after the fields. For a value that is a property of the
//!   theme rather than a knob on it (idea's `radius-pill` is always
//!   `Length::Full`): a stored field would invite setting it to something
//!   meaningless, and an extra struct field would break every literal
//!   that builds the struct.
//!
//! Field attributes:
//!
//! - `skip` — not a token: kept on the struct, absent from the vocabulary;
//! - `rename = "name"` — the field is reached as `t.name` and its token
//!   segment is `name` (idea's `colors` field is the `color` namespace).
//!   One attribute moves both because the accessor path IS the token name;
//!   renaming only one of them would let `t.x.y()` name a token not
//!   spelled `x-y`;
//! - `as = Kind` — a leaf that installs as registry kind `Kind` (`Color`,
//!   `Length`, `f32`), converted with `From` (idea stores spacing as `f32`
//!   and installs it as `Length::Px`);
//! - `flatten` — embed another token set: its tokens keep their own names
//!   and accessor paths (`t.color.surface()` reaches the embedded set
//!   through `Deref`), the embedding set adds its own. One per struct.
//!   A name or accessor shared with the embedded set is a compile error;
//! - `app_background` — this `Color` leaf themes the host surface; one per
//!   theme, on the theme's own fields.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Data, DeriveInput, Fields, LitStr};

/// One `computed(name: Type = path)` entry.
struct Computed {
    name: syn::Ident,
    ty: syn::Type,
    func: syn::Path,
}

impl Parse for Computed {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let name = input.parse()?;
        input.parse::<syn::Token![:]>()?;
        let ty = input.parse()?;
        input.parse::<syn::Token![=]>()?;
        let func = input.parse()?;
        Ok(Computed { name, ty, func })
    }
}

struct StructAttrs {
    default: Option<syn::Path>,
    prefix: Option<LitStr>,
    vocabulary: Option<LitStr>,
    as_kind: Option<syn::Type>,
    computed: Vec<Computed>,
}

#[derive(Default)]
struct FieldAttrs {
    skip: bool,
    app_background: bool,
    flatten: bool,
    rename: Option<LitStr>,
    as_kind: Option<syn::Type>,
}

fn struct_attrs(input: &DeriveInput) -> syn::Result<StructAttrs> {
    let mut out = StructAttrs { default: None, prefix: None, vocabulary: None, as_kind: None, computed: Vec::new() };
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
            } else if meta.path.is_ident("vocabulary") {
                out.vocabulary = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("as") {
                out.as_kind = Some(meta.value()?.parse()?);
                Ok(())
            } else if meta.path.is_ident("computed") {
                let content;
                syn::parenthesized!(content in meta.input);
                let list = Punctuated::<Computed, syn::Token![,]>::parse_terminated(&content)?;
                out.computed.extend(list);
                Ok(())
            } else {
                Err(meta.error(
                    "unknown `tokens` attribute on a struct; expected `default = path::to::fn`, `prefix = \"…\"`, \
                     `vocabulary = \"…\"`, `as = Kind` or `computed(name: Type = path::to::fn)`",
                ))
            }
        })?;
    }
    if out.default.is_none() {
        for (lit, what) in [(&out.prefix, "prefix"), (&out.vocabulary, "vocabulary")] {
            if let Some(lit) = lit {
                return Err(syn::Error::new(
                    lit.span(),
                    format!("`{what}` applies to a theme; add `default = path::to::fn` to make this struct a theme"),
                ));
            }
        }
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
            } else if meta.path.is_ident("flatten") {
                out.flatten = true;
                Ok(())
            } else if meta.path.is_ident("rename") {
                let lit: LitStr = meta.value()?.parse()?;
                if syn::parse_str::<syn::Ident>(&lit.value()).is_err() || lit.value().starts_with("r#") {
                    return Err(syn::Error::new(lit.span(), "`rename` takes the accessor name to use — a plain identifier (`color`)"));
                }
                out.rename = Some(lit);
                Ok(())
            } else if meta.path.is_ident("as") {
                out.as_kind = Some(meta.value()?.parse()?);
                Ok(())
            } else {
                Err(meta.error(
                    "unknown `tokens` attribute on a field; expected `skip`, `rename = \"…\"`, `as = Kind`, \
                     `flatten` or `app_background`",
                ))
            }
        })?;
    }
    let span = field.span();
    if out.skip && (out.app_background || out.flatten || out.rename.is_some() || out.as_kind.is_some()) {
        return Err(syn::Error::new(span, "a `skip`ped field is not a token; it takes no other `tokens` attribute"));
    }
    if out.flatten && (out.rename.is_some() || out.as_kind.is_some() || out.app_background) {
        return Err(syn::Error::new(
            span,
            "a `flatten`ed field contributes the embedded set's own names — it takes no `rename`, `as` or `app_background`",
        ));
    }
    Ok(out)
}

/// `soft_bg` → (`soft-bg`, `soft_bg`); a raw identifier loses its `r#`.
fn segments(name: &str) -> (String, String) {
    let path = name.trim_start_matches("r#").to_string();
    (path.replace('_', "-"), path)
}

/// Where an accessor's token(s) come from.
enum Source<'a> {
    /// A field dispatched through `TokenField` (a leaf or a nested set).
    Field(&'a syn::Field),
    /// A leaf field converted to registry kind `.1`.
    As(&'a syn::Field, syn::Type),
    /// A flattened set.
    Flatten(&'a syn::Field),
    /// A computed token.
    Computed(&'a Computed),
}

struct Entry<'a> {
    source: Source<'a>,
    /// What the accessor is called (the rename, else the field name).
    accessor: syn::Ident,
    vis: syn::Visibility,
    offset: TokenStream2,
    docs: Vec<&'a syn::Attribute>,
    app_background: bool,
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
    let accessor_ty = format_ident!("{}Tokens", name);

    // The accessors, in declaration order (fields, then computed tokens),
    // each with its offset into this set's leaves: the sum of the
    // preceding entries' leaf counts.
    let mut entries: Vec<Entry> = Vec::new();
    let mut running = quote!(0usize);
    let mut flatten_seen: Option<&syn::Field> = None;
    for field in fields {
        let fa = field_attrs(field)?;
        if fa.skip {
            continue;
        }
        let ident = field.ident.as_ref().expect("named field");
        let ty = &field.ty;
        let accessor = match &fa.rename {
            Some(lit) => syn::Ident::new(&lit.value(), lit.span()),
            None => ident.clone(),
        };
        let source = if fa.flatten {
            if flatten_seen.is_some() {
                return Err(syn::Error::new(
                    field.span(),
                    "only one field can be `flatten`ed (its accessors are reached through `Deref`)",
                ));
            }
            flatten_seen = Some(field);
            Source::Flatten(field)
        } else if let Some(kind) = fa.as_kind.clone().or_else(|| attrs.as_kind.clone()) {
            Source::As(field, kind)
        } else {
            Source::Field(field)
        };
        let count = match &source {
            Source::As(..) => quote!(1usize),
            _ => quote!(<#ty as #rv::TokenField>::LEAF_COUNT),
        };
        entries.push(Entry {
            source,
            accessor,
            vis: field.vis.clone(),
            offset: running.clone(),
            docs: field.attrs.iter().filter(|a| a.path().is_ident("doc")).collect(),
            app_background: fa.app_background,
        });
        running = quote!(#running + #count);
    }
    for c in &attrs.computed {
        entries.push(Entry {
            source: Source::Computed(c),
            accessor: c.name.clone(),
            vis: syn::parse_quote!(pub),
            offset: running.clone(),
            docs: Vec::new(),
            app_background: false,
        });
        running = quote!(#running + 1usize);
    }
    let leaf_count = running;

    // Accessor names are unique among this struct's own entries (a rename
    // or a computed token can collide with a field).
    for (i, e) in entries.iter().enumerate() {
        if entries[..i].iter().any(|p| p.accessor == e.accessor) {
            return Err(syn::Error::new(
                e.accessor.span(),
                format!("two accessors of this token set are both named `{}`", e.accessor),
            ));
        }
    }

    let backgrounds: Vec<&Entry> = entries.iter().filter(|e| e.app_background).collect();
    if backgrounds.len() > 1 {
        return Err(syn::Error::new(backgrounds[1].accessor.span(), "only one field can be the `app_background`"));
    }
    if let (Some(bg), None) = (backgrounds.first(), &attrs.default) {
        return Err(syn::Error::new(
            bg.accessor.span(),
            "`app_background` belongs on a field of the theme root — the struct with `#[tokens(default = …)]`",
        ));
    }

    let mut nodes = Vec::new();
    let mut slot_fields = Vec::new();
    let mut slot_inits = Vec::new();
    let mut methods = Vec::new();
    let mut collects = Vec::new();
    let mut checks = Vec::new();
    let mut deref = TokenStream2::new();
    let mut embeds = TokenStream2::new();
    for e in &entries {
        let (acc, fvis, docs, offset) = (&e.accessor, &e.vis, &e.docs, &e.offset);
        let (kebab, path) = segments(&acc.to_string());
        match &e.source {
            Source::Field(field) | Source::Flatten(field) => {
                let flatten = matches!(e.source, Source::Flatten(_));
                let (ident, ty) = (field.ident.as_ref().unwrap(), &field.ty);
                let (kebab, path) = if flatten { (String::new(), String::new()) } else { (kebab, path) };
                nodes.push(quote_spanned! {ty.span()=>
                    #rv::TokenNode {
                        name: #kebab,
                        path: #path,
                        kind: <#ty as #rv::TokenField>::KIND,
                        children: <#ty as #rv::TokenField>::NODES,
                        flatten: #flatten,
                    }
                });
                slot_fields.push(quote! { #(#docs)* #fvis #acc: <#ty as #rv::TokenField>::Slot<R>, });
                slot_inits.push(quote! { #acc: <#ty as #rv::TokenField>::slot::<R>(index + #offset), });
                methods.push(quote! {
                    #(#docs)*
                    #[inline]
                    #fvis fn #acc(&self) -> <#ty as #rv::TokenField>::Read<R> {
                        <#ty as #rv::TokenField>::read::<R>(self.__index + #offset)
                    }
                });
                collects.push(quote! { #rv::TokenField::collect_values(&self.#ident, out); });
                // A type that is neither a leaf nor a set fails here,
                // spanned on the field, with `TokenField`'s explanation.
                checks.push(quote_spanned! {ty.span()=> __is_token_field::<#ty>(); });
                if flatten {
                    embeds = quote! {
                        out.push(::core::any::TypeId::of::<#ty>());
                        <#ty as #rv::TokenField>::embedded_sets(out);
                    };
                    deref = quote! {
                        /// The flattened set's accessors are this set's too.
                        impl<R: #rv::TokenTheme> ::core::ops::Deref for #accessor_ty<R> {
                            type Target = <#ty as #rv::TokenField>::Slot<R>;
                            fn deref(&self) -> &Self::Target {
                                &self.#acc
                            }
                        }
                    };
                }
            }
            Source::As(field, kind) => {
                let (ident, ty) = (field.ident.as_ref().unwrap(), &field.ty);
                nodes.push(quote_spanned! {kind.span()=>
                    #rv::TokenNode {
                        name: #kebab,
                        path: #path,
                        kind: ::core::option::Option::Some(<#kind as #rv::TokenPrimitive>::KIND),
                        children: &[],
                        flatten: false,
                    }
                });
                slot_fields.push(quote! { #(#docs)* #fvis #acc: #rv::LeafSlot, });
                slot_inits.push(quote! { #acc: #rv::LeafSlot, });
                methods.push(quote! {
                    #(#docs)*
                    #[inline]
                    #fvis fn #acc(&self) -> ::runtime_vocabulary::glue::Tokenized<#kind> {
                        #rv::__read_leaf::<R, #kind>(self.__index + #offset)
                    }
                });
                collects.push(quote_spanned! {ty.span()=>
                    out.push(#rv::TokenPrimitive::into_token_value(
                        <#kind as ::core::convert::From<#ty>>::from(::core::clone::Clone::clone(&self.#ident)),
                    ));
                });
            }
            Source::Computed(c) => {
                let (ty, func) = (&c.ty, &c.func);
                nodes.push(quote_spanned! {ty.span()=>
                    #rv::TokenNode {
                        name: #kebab,
                        path: #path,
                        kind: <#ty as #rv::TokenField>::KIND,
                        children: &[],
                        flatten: false,
                    }
                });
                let doc = format!("Computed token: `{}(&self)`.", quote!(#func).to_string().replace(' ', ""));
                slot_fields.push(quote! { #[doc = #doc] pub #acc: #rv::LeafSlot, });
                slot_inits.push(quote! { #acc: #rv::LeafSlot, });
                methods.push(quote! {
                    #[doc = #doc]
                    #[inline]
                    pub fn #acc(&self) -> ::runtime_vocabulary::glue::Tokenized<<#ty as #rv::TokenLeaf>::Value> {
                        #rv::__read_leaf::<R, <#ty as #rv::TokenLeaf>::Value>(self.__index + #offset)
                    }
                });
                collects.push(quote_spanned! {func.span()=>
                    {
                        let value: #ty = #func(self);
                        out.push(#rv::TokenPrimitive::into_token_value(#rv::TokenLeaf::token_value(&value)));
                    }
                });
                checks.push(quote_spanned! {ty.span()=> __is_token_leaf::<#ty>(); });
            }
        }
    }

    // A flattened field must be a SET, and no accessor of ours may hide one
    // of its own.
    let flatten_checks = flatten_seen.map(|field| {
        let ty = &field.ty;
        let own: Vec<String> = entries
            .iter()
            .filter(|e| !matches!(e.source, Source::Flatten(_)))
            .map(|e| segments(&e.accessor.to_string()).1)
            .collect();
        quote_spanned! {ty.span()=>
            const _: () = {
                if <#ty as #rv::TokenField>::KIND.is_some() {
                    panic!("`flatten` embeds a token SET (a `#[derive(Tokens)]` struct), not a leaf");
                }
                #rv::__assert_no_shadow(&[#(#own),*], <#ty as #rv::TokenField>::NODES);
            };
        }
    });

    let accessor_doc = format!(
        "The token accessors of [`{name}`]: what a `stylesheet!` block binding is for a sheet \
         declared over a theme containing it. A leaf is read by calling it (`t.surface()`); a \
         nested set is reached through its field (`t.accent.soft_bg()`)."
    );
    let debug_name = accessor_ty.to_string();

    let mut out = quote! {
        #[doc = #accessor_doc]
        #vis struct #accessor_ty<R: #rv::TokenTheme> {
            #[doc(hidden)]
            __index: usize,
            #[doc(hidden)]
            __root: ::core::marker::PhantomData<fn() -> R>,
            #(#slot_fields)*
        }

        impl<R: #rv::TokenTheme> #accessor_ty<R> {
            #(#methods)*
        }

        #deref

        impl<R: #rv::TokenTheme> ::core::clone::Clone for #accessor_ty<R> {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl<R: #rv::TokenTheme> ::core::marker::Copy for #accessor_ty<R> {}

        impl<R: #rv::TokenTheme> ::core::fmt::Debug for #accessor_ty<R> {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(#debug_name).finish_non_exhaustive()
            }
        }

        impl<R: #rv::TokenTheme> ::core::default::Default for #accessor_ty<R> {
            /// The accessors of a theme ROOT (its tokens start at index 0).
            fn default() -> Self {
                <#name as #rv::TokenField>::slot::<R>(0)
            }
        }

        impl #rv::TokenField for #name {
            const LEAF_COUNT: usize = #leaf_count;
            const KIND: ::core::option::Option<&'static str> = ::core::option::Option::None;
            const NODES: &'static [#rv::TokenNode] = &[#(#nodes),*];
            type Read<R: #rv::TokenTheme> = #accessor_ty<R>;
            type Slot<R: #rv::TokenTheme> = #accessor_ty<R>;

            fn read<R: #rv::TokenTheme>(index: usize) -> #accessor_ty<R> {
                <Self as #rv::TokenField>::slot::<R>(index)
            }

            fn slot<R: #rv::TokenTheme>(index: usize) -> #accessor_ty<R> {
                #accessor_ty {
                    __index: index,
                    __root: ::core::marker::PhantomData,
                    #(#slot_inits)*
                }
            }

            fn collect_values(&self, out: &mut ::std::vec::Vec<::runtime_vocabulary::glue::TokenValue>) {
                #(#collects)*
            }

            fn embedded_sets(out: &mut ::std::vec::Vec<::core::any::TypeId>) {
                let _ = &out;
                #embeds
            }
        }

        impl #rv::Tokens for #name {}

        const _: () = {
            #[allow(dead_code)]
            fn __is_token_field<T: #rv::TokenField + ?Sized>() {}
            #[allow(dead_code)]
            fn __is_token_leaf<T: #rv::TokenLeaf>() {}
            #[allow(dead_code)]
            fn __check() {
                #(#checks)*
            }
        };

        #flatten_checks
    };

    if let Some(default) = &attrs.default {
        let prefix = attrs.prefix.as_ref().map(LitStr::value).unwrap_or_default();
        let vocabulary = attrs.vocabulary.as_ref().map(LitStr::value).unwrap_or_else(|| name.to_string());
        let app_background = match backgrounds.first() {
            Some(bg) => {
                let offset = &bg.offset;
                quote!(::core::option::Option::Some(#offset))
            }
            None => quote!(::core::option::Option::None),
        };
        let background_check = backgrounds.first().map(|bg| {
            let (value_ty, span) = match &bg.source {
                Source::Field(f) => {
                    let ty = &f.ty;
                    (quote!(<#ty as #rv::TokenLeaf>::Value), ty.span())
                }
                Source::As(_, kind) => (quote!(#kind), kind.span()),
                Source::Flatten(_) | Source::Computed(_) => unreachable!("rejected by field_attrs"),
            };
            quote_spanned! {span=>
                const _: () = {
                    fn __is_color<T: ::runtime_vocabulary::tokens::__IsColor>() {}
                    #[allow(dead_code)]
                    fn __check() { __is_color::<#value_ty>(); }
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
                const NAME: &'static str = #vocabulary;
                fn default_theme() -> Self {
                    #default()
                }
            }

            impl ::runtime_vocabulary::glue::TokenVocabulary for #name {
                type Tokens = #accessor_ty<#name>;
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
        assert!(out.contains("const NAME : & 'static str = \"App\""), "{out}");
    }

    #[test]
    fn vocabulary_names_the_catalog_entry() {
        let out = expand("#[tokens(default = light, vocabulary = \"IdeaThemeRef\")] struct T { a: Color }").unwrap();
        assert!(out.contains("const NAME : & 'static str = \"IdeaThemeRef\""), "{out}");
    }

    #[test]
    fn rename_changes_the_accessor_and_the_name_segment() {
        let out = expand("struct T { #[tokens(rename = \"color\")] colors: Colors }").unwrap();
        assert!(out.contains("name : \"color\""), "{out}");
        assert!(out.contains("fn color (& self)"), "{out}");
        assert!(out.contains("& self . colors"), "values still read the real field: {out}");
        assert!(!out.contains("fn colors"), "{out}");
    }

    #[test]
    fn as_installs_a_field_as_another_kind() {
        let out = expand("#[tokens(as = Length)] struct Spacing { md: f32 }").unwrap();
        assert!(out.contains("< Length as :: runtime_vocabulary :: tokens :: TokenPrimitive > :: KIND"), "{out}");
        assert!(out.contains("Tokenized < Length >"), "{out}");
        assert!(out.contains(":: core :: convert :: From < f32 >"), "{out}");
    }

    #[test]
    fn computed_tokens_follow_the_fields() {
        let out = expand("#[tokens(computed(pill: Length = radius_pill))] struct Radius { sm: f32 }").unwrap();
        assert!(out.contains("name : \"pill\""), "{out}");
        assert!(out.contains("radius_pill (self)"), "{out}");
        assert!(out.find("name : \"sm\"").unwrap() < out.find("name : \"pill\"").unwrap(), "{out}");
    }

    #[test]
    fn flatten_derefs_to_the_embedded_set() {
        let out = expand("struct T { own: Color, #[tokens(flatten)] base: Base }").unwrap();
        assert!(out.contains("flatten : true"), "{out}");
        assert!(out.contains("Deref for TTokens"), "{out}");
        assert!(out.contains("__assert_no_shadow (& [\"own\"]"), "{out}");
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
            ("#[tokens(vocabulary = \"X\")] struct T { a: Color }", "add `default"),
            ("#[tokens(default = f, prefix = \"Bad_Prefix\")] struct T { a: Color }", "kebab"),
            ("#[tokens(bogus)] struct T { a: Color }", "unknown `tokens` attribute"),
            ("struct T { #[tokens(app_background)] a: Color }", "theme root"),
            (
                "#[tokens(default = f)] struct T { #[tokens(app_background)] a: Color, #[tokens(app_background)] b: Color }",
                "only one field",
            ),
            ("struct T { #[tokens(flatten)] a: A, #[tokens(flatten)] b: B }", "only one field can be `flatten`ed"),
            ("struct T { #[tokens(flatten, rename = \"x\")] a: A }", "takes no `rename`"),
            ("struct T { #[tokens(rename = \"a-b\")] a: A }", "plain identifier"),
            ("struct T { #[tokens(rename = \"b\")] a: Color, b: Color }", "both named `b`"),
            ("#[tokens(computed(a: Length = f))] struct T { a: Color }", "both named `a`"),
            ("struct T { #[tokens(skip, as = Length)] a: f32 }", "no other `tokens` attribute"),
        ] {
            let err = expand(src).expect_err(src);
            assert!(err.contains(msg), "`{src}` → {err}");
        }
    }
}
