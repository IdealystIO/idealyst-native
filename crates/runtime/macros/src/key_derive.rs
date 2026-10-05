//! `#[derive(Key)]`: a struct or enum a generic `#[host_fn]` can compare,
//! hash and clone without knowing its type
//! (`runtime_vocabulary::host_types::Key`).
//!
//! It emits `PartialEq`, `Eq`, `PartialOrd`, `Ord` and `Hash` with the
//! `Key` encoding, all from the same fields in the same order: fields in
//! declaration order, an enum's variant (declaration index) first. The
//! order is the one `#[derive(PartialOrd, Ord)]` gives, and the encoding is
//! built so its bytes compare the same way — so the app, sorting a bundle's
//! keys as bytes, agrees with the bundle sorting them natively. Writing or
//! deriving any of those traits as well is a conflicting-impl error, which
//! is the point: a hand-written `Ord` would disagree with the bytes.
//!
//! An `f32`/`f64` field (spelled so) compares with `total_cmp`, hashes its
//! bits, and encodes in `total_cmp` order.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{Data, DeriveInput, Fields, Type};

enum Float {
    No,
    F32,
    F64,
}

fn float(ty: &Type) -> Float {
    match ty {
        Type::Path(p) if p.qself.is_none() && p.path.segments.len() == 1 => {
            let id = &p.path.segments[0].ident;
            if id == "f64" {
                Float::F64
            } else if id == "f32" {
                Float::F32
            } else {
                Float::No
            }
        }
        _ => Float::No,
    }
}

/// The bindings a pattern over `fields` introduces (prefixed), and the
/// pattern.
fn bind(fields: &Fields, prefix: &str) -> (Vec<syn::Ident>, TokenStream2) {
    match fields {
        Fields::Named(f) => {
            let (names, binds): (Vec<_>, Vec<_>) = f
                .named
                .iter()
                .map(|f| {
                    let n = f.ident.clone().expect("named");
                    let b = format_ident!("{}_{}", prefix, n);
                    (b.clone(), quote! { #n: #b })
                })
                .unzip();
            (names, quote! { { #(#binds),* } })
        }
        Fields::Unnamed(f) => {
            let names: Vec<_> = (0..f.unnamed.len()).map(|i| format_ident!("{}_{}", prefix, i)).collect();
            (names.clone(), quote! { ( #(#names),* ) })
        }
        Fields::Unit => (Vec::new(), quote! {}),
    }
}

fn field_types(fields: &Fields) -> Vec<&Type> {
    match fields {
        Fields::Named(f) => f.named.iter().map(|f| &f.ty).collect(),
        Fields::Unnamed(f) => f.unnamed.iter().map(|f| &f.ty).collect(),
        Fields::Unit => Vec::new(),
    }
}

pub(crate) fn derive(input: DeriveInput) -> syn::Result<TokenStream2> {
    let k = quote!(::runtime_vocabulary::host_types);
    let name = &input.ident;

    // Each type parameter must itself be a Key.
    let mut generics = input.generics.clone();
    for p in generics.type_params_mut() {
        p.bounds.push(syn::parse_quote!(#k::Key));
    }
    let (impl_g, ty_g, where_g) = generics.split_for_impl();

    // One arm per variant (a struct is one unnamed "variant").
    struct Arm<'a> {
        path: TokenStream2,
        fields: &'a Fields,
    }
    let arms: Vec<Arm> = match &input.data {
        Data::Struct(s) => vec![Arm { path: quote!(#name), fields: &s.fields }],
        Data::Enum(e) => e
            .variants
            .iter()
            .map(|v| {
                let id = &v.ident;
                Arm { path: quote!(#name::#id), fields: &v.fields }
            })
            .collect(),
        Data::Union(u) => return Err(syn::Error::new_spanned(u.union_token, "#[derive(Key)] can't be a union")),
    };
    let is_enum = matches!(input.data, Data::Enum(_));
    if is_enum && arms.is_empty() {
        return Err(syn::Error::new_spanned(name, "#[derive(Key)] needs at least one variant"));
    }
    let wide = arms.len() > 256;

    let cmp_field = |ty: &Type, a: &syn::Ident, b: &syn::Ident| match float(ty) {
        Float::F64 => quote!(#k::float::cmp_f64(#a, #b)),
        Float::F32 => quote!(#k::float::cmp_f32(#a, #b)),
        Float::No => quote!(::core::cmp::Ord::cmp(#a, #b)),
    };
    let hash_field = |ty: &Type, a: &syn::Ident| match float(ty) {
        Float::F64 => quote!(#k::float::hash_f64(#a, __h);),
        Float::F32 => quote!(#k::float::hash_f32(#a, __h);),
        Float::No => quote!(::core::hash::Hash::hash(#a, __h);),
    };
    let encode_field = |ty: &Type, a: &syn::Ident| match float(ty) {
        Float::F64 => quote!(#k::float::encode_f64(*#a, __out);),
        Float::F32 => quote!(#k::float::encode_f32(*#a, __out);),
        Float::No => quote!(#k::Key::encode_key(#a, __out);),
    };
    let decode_field = |ty: &Type| match float(ty) {
        Float::F64 => quote!(#k::float::decode_f64(__input)?),
        Float::F32 => quote!(#k::float::decode_f32(__input)?),
        Float::No => quote!(<#ty as #k::Key>::decode_key(__input)?),
    };

    let mut cmp_arms = Vec::new();
    let mut index_arms = Vec::new();
    let mut hash_arms = Vec::new();
    let mut encode_arms = Vec::new();
    let mut decode_arms = Vec::new();
    for (i, arm) in arms.iter().enumerate() {
        let path = &arm.path;
        let types = field_types(arm.fields);
        let (a, pat_a) = bind(arm.fields, "__a");
        let (b, pat_b) = bind(arm.fields, "__b");
        let idx = i as u32;

        let cmps = types.iter().zip(a.iter().zip(&b)).map(|(ty, (a, b))| {
            let c = cmp_field(ty, a, b);
            quote! { match #c { ::core::cmp::Ordering::Equal => {}, __o => return __o } }
        });
        cmp_arms.push(quote! { (#path #pat_a, #path #pat_b) => { #(#cmps)* } });
        index_arms.push(quote! { #path { .. } => #idx });

        let hashes = types.iter().zip(&a).map(|(ty, a)| hash_field(ty, a));
        let hash_idx = is_enum.then(|| quote! { ::core::hash::Hash::hash(&#idx, __h); });
        hash_arms.push(quote! { #path #pat_a => { #hash_idx #(#hashes)* } });

        let tag = if !is_enum {
            quote!()
        } else if wide {
            quote! { __out.extend_from_slice(&#idx.to_be_bytes()); }
        } else {
            let b = idx as u8;
            quote! { __out.push(#b); }
        };
        let encodes = types.iter().zip(&a).map(|(ty, a)| encode_field(ty, a));
        encode_arms.push(quote! { #path #pat_a => { #tag #(#encodes)* } });

        let build = match arm.fields {
            Fields::Named(f) => {
                let parts = f.named.iter().map(|f| {
                    let n = f.ident.as_ref().expect("named");
                    let d = decode_field(&f.ty);
                    quote! { #n: #d }
                });
                quote! { #path { #(#parts),* } }
            }
            Fields::Unnamed(f) => {
                let parts = f.unnamed.iter().map(|f| decode_field(&f.ty));
                quote! { #path( #(#parts),* ) }
            }
            Fields::Unit => quote! { #path },
        };
        decode_arms.push((idx, build));
    }

    let (decode_idx, decode_build): (Vec<u32>, Vec<TokenStream2>) = decode_arms.into_iter().unzip();

    // Every key's length, when all have one: the sum of the fields' (an
    // enum's when every variant's is the same, plus its tag).
    let field_width = |ty: &Type| match float(ty) {
        Float::F64 => quote!(::core::option::Option::Some(8usize)),
        Float::F32 => quote!(::core::option::Option::Some(4usize)),
        Float::No => quote!(<#ty as #k::Key>::WIDTH),
    };
    let variant_width = |fields: &Fields| {
        let parts = field_types(fields).into_iter().map(|t| field_width(t));
        quote! {{
            let __w = ::core::option::Option::Some(0usize);
            #( let __w = #k::width_sum(__w, #parts); )*
            __w
        }}
    };
    let width = if is_enum {
        let tag = if wide { 4usize } else { 1usize };
        let first = variant_width(arms[0].fields);
        let rest = arms[1..].iter().map(|a| variant_width(a.fields));
        quote! {{
            let __w = #first;
            #( let __w = #k::width_same(__w, #rest); )*
            #k::width_sum(__w, ::core::option::Option::Some(#tag))
        }}
    } else {
        variant_width(arms[0].fields)
    };
    let (cmp_body, hash_body, encode_body, decode_body) = if is_enum {
        let read_tag = if wide {
            quote! { <u32 as #k::Key>::decode_key(__input)? }
        } else {
            quote! { <u8 as #k::Key>::decode_key(__input)? as u32 }
        };
        let name_str = name.to_string();
        (
            quote! {
                fn __index #impl_g (v: &#name #ty_g) -> u32 #where_g { match v { #(#index_arms),* } }
                match (self, other) {
                    #(#cmp_arms)*
                    _ => return ::core::cmp::Ord::cmp(&__index(self), &__index(other)),
                }
                ::core::cmp::Ordering::Equal
            },
            quote! { match self { #(#hash_arms)* } },
            quote! { match self { #(#encode_arms)* } },
            quote! {
                match #read_tag {
                    #(#decode_idx => ::core::result::Result::Ok(#decode_build),)*
                    __t => ::core::result::Result::Err(::std::format!("key: {} has no variant {}", #name_str, __t)),
                }
            },
        )
    } else {
        let arm = &arms[0];
        let path = &arm.path;
        let types = field_types(arm.fields);
        let (a, pat_a) = bind(arm.fields, "__a");
        let (b, pat_b) = bind(arm.fields, "__b");
        let cmps = types.iter().zip(a.iter().zip(&b)).map(|(ty, (a, b))| {
            let c = cmp_field(ty, a, b);
            quote! { match #c { ::core::cmp::Ordering::Equal => {}, __o => return __o } }
        });
        let hashes = types.iter().zip(&a).map(|(ty, a)| hash_field(ty, a));
        let encodes = types.iter().zip(&a).map(|(ty, a)| encode_field(ty, a));
        let build = &decode_build[0];
        let decode = quote! { ::core::result::Result::Ok(#build) };
        (
            quote! {
                let #path #pat_a = self;
                let #path #pat_b = other;
                #(#cmps)*
                ::core::cmp::Ordering::Equal
            },
            quote! { let #path #pat_a = self; #(#hashes)* },
            quote! { let #path #pat_a = self; #(#encodes)* },
            decode,
        )
    };

    Ok(quote! {
        impl #impl_g ::core::cmp::Ord for #name #ty_g #where_g {
            #[allow(unused_variables)]
            fn cmp(&self, other: &Self) -> ::core::cmp::Ordering { #cmp_body }
        }
        impl #impl_g ::core::cmp::PartialOrd for #name #ty_g #where_g {
            fn partial_cmp(&self, other: &Self) -> ::core::option::Option<::core::cmp::Ordering> {
                ::core::option::Option::Some(::core::cmp::Ord::cmp(self, other))
            }
        }
        impl #impl_g ::core::cmp::PartialEq for #name #ty_g #where_g {
            fn eq(&self, other: &Self) -> bool {
                ::core::cmp::Ord::cmp(self, other) == ::core::cmp::Ordering::Equal
            }
        }
        impl #impl_g ::core::cmp::Eq for #name #ty_g #where_g {}
        impl #impl_g ::core::hash::Hash for #name #ty_g #where_g {
            #[allow(unused_variables)]
            fn hash<__H: ::core::hash::Hasher>(&self, __h: &mut __H) { #hash_body }
        }
        impl #impl_g #k::Key for #name #ty_g #where_g {
            const WIDTH: ::core::option::Option<usize> = #width;
            #[allow(unused_variables)]
            fn encode_key(&self, __out: &mut ::std::vec::Vec<u8>) { #encode_body }
            #[allow(unused_variables)]
            fn decode_key(__input: &mut &[u8]) -> ::core::result::Result<Self, ::std::string::String> { #decode_body }
        }
    })
}
