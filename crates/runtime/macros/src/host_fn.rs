//! `#[host_fn]` — an app function remote code can call: in the app the
//! function itself, in a remote bundle a stub that asks the app to run it.
//! One definition, in a crate both builds compile (an app, or a library
//! like idea-ui).
//!
//! - **App build:** the function, unchanged. When the app hosts remote
//!   components, also `<name>::export()`, the `HostFnDef` the app lists in
//!   its allowlist (`stream_host::remote::install_with`).
//! - **Bundle build:** a stub. A sync fn keeps its signature; its body
//!   becomes one wasm import call. An `async fn` returns a
//!   `runtime_vocabulary::remote::bundle::HostFuture<Output>`, which the
//!   framework's `spawn_then(future, then)` drives as it does natively.
//!
//! Arguments and the result cross as `RemoteValue`s — plain data, including
//! values that cross by key (an idea-theme `ToneRef`), which only the app
//! decodes. Each stub links an import named `<module_path>::<fn>#<schema>`,
//! the schema a fingerprint of the signature, checked at load.
//!
//! Which build is decided by the vocabulary (`__remote_guest_split!`,
//! `__remote_enabled!`), so the defining crate declares no cfg and an app
//! without remote components gets only the function.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, Pat, ReturnType, Type};

pub(crate) fn expand(func: ItemFn) -> syn::Result<TokenStream2> {
    let sig = &func.sig;
    let name = &sig.ident;
    let vis = &func.vis;
    let is_async = sig.asyncness.is_some();
    if !sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(&sig.generics, "#[host_fn] can't be generic: the signature is a wire contract"));
    }
    let mut arg_names = Vec::new();
    let mut arg_types: Vec<&Type> = Vec::new();
    for arg in &sig.inputs {
        match arg {
            FnArg::Receiver(r) => return Err(syn::Error::new_spanned(r, "#[host_fn] must be a free function")),
            FnArg::Typed(pt) => match &*pt.pat {
                Pat::Ident(pi) => {
                    arg_names.push(pi.ident.clone());
                    arg_types.push(&pt.ty);
                }
                other => return Err(syn::Error::new_spanned(other, "#[host_fn] arguments must be plain identifiers")),
            },
        }
    }
    let ret: TokenStream2 = match &sig.output {
        ReturnType::Default => quote!(()),
        ReturnType::Type(_, ty) => quote!(#ty),
    };

    // The type spellings, hashed with FNV-1a: app and bundle may be built
    // by different toolchains, and std's `DefaultHasher` promises no
    // stability across Rust releases. Each part is length-prefixed so
    // `(ab, c)` and `(a, bc)` differ.
    let schema: u64 = {
        let mut parts: Vec<String> = arg_types.iter().map(|ty| quote!(#ty).to_string()).collect();
        parts.push(ret.to_string());
        parts.push(if is_async { "async" } else { "sync" }.to_string());
        fnv1a(&parts)
    };
    let schema_hex = format!("{schema:016x}");
    let v = quote!(::runtime_vocabulary::remote);

    let call_ident = format_ident!("__host_fn_call_{}", name);
    let export_ident = format_ident!("__host_fn_export_{}", name);
    // A bundle's arguments are untrusted: one that does not decode is an
    // `Err`, which stops that bundle (never a panic in the app).
    let decode_args = quote! {
        let mut __input: &[u8] = &__args;
        #(
            let #arg_names: #arg_types = <#arg_types as #v::RemoteValue>::decode(&mut __input).map_err(|e| {
                ::std::format!(
                    "host_fn `{}`: argument `{}` does not decode ({})",
                    ::core::stringify!(#name), ::core::stringify!(#arg_names), e
                )
            })?;
        )*
    };
    let encode_reply = |value: TokenStream2| {
        quote! {{
            let mut __out = ::std::vec::Vec::new();
            #v::RemoteValue::encode(&#value, &mut __out);
            __out
        }}
    };

    let (call_fn, kind) = if is_async {
        let reply = encode_reply(quote!(#name(#(#arg_names),*).await));
        (
            quote! {
                fn #call_ident(__args: ::std::vec::Vec<u8>) -> ::core::result::Result<
                    ::std::pin::Pin<::std::boxed::Box<dyn ::std::future::Future<Output = ::std::vec::Vec<u8>>>>,
                    ::std::string::String,
                > {
                    #decode_args
                    ::core::result::Result::Ok(::std::boxed::Box::pin(async move { #reply }))
                }
            },
            quote!(#v::host_fn::HostFnKind::Async(#call_ident)),
        )
    } else {
        let reply = encode_reply(quote!(#name(#(#arg_names),*)));
        (
            quote! {
                fn #call_ident(__args: &[u8]) -> ::core::result::Result<::std::vec::Vec<u8>, ::std::string::String> {
                    #decode_args
                    ::core::result::Result::Ok(#reply)
                }
            },
            quote!(#v::host_fn::HostFnKind::Sync(#call_ident)),
        )
    };

    let attrs = &func.attrs;
    let block = &func.block;
    let inputs = &sig.inputs;
    let encode_args = quote! {
        let mut __args = ::std::vec::Vec::new();
        #( #v::RemoteValue::encode(&#arg_names, &mut __args); )*
    };

    let stub = if is_async {
        quote! {
            #(#attrs)*
            #vis fn #name(#inputs) -> #v::bundle::HostFuture<#ret> {
                #[link(wasm_import_module = "idealyst_host_fn")]
                unsafe extern "C" {
                    #[link_name = concat!(module_path!(), "::", stringify!(#name), "#", #schema_hex)]
                    fn __import(args_ptr: *const u8, args_len: u32, then: u32);
                }
                #encode_args
                fn __decode(mut b: &[u8]) -> ::core::option::Option<#ret> {
                    <#ret as #v::RemoteValue>::decode(&mut b).ok()
                }
                // SAFETY: the app copies `args_len` bytes at `args_ptr`
                // during the call.
                #v::bundle::HostFuture::start(stringify!(#name), __args, __decode, |ptr, len, then| unsafe {
                    __import(ptr, len, then)
                })
            }
        }
    } else {
        quote! {
            #(#attrs)*
            #vis fn #name(#inputs) -> #ret {
                #[link(wasm_import_module = "idealyst_host_fn")]
                unsafe extern "C" {
                    #[link_name = concat!(module_path!(), "::", stringify!(#name), "#", #schema_hex)]
                    fn __import(args_ptr: *const u8, args_len: u32) -> i64;
                }
                #encode_args
                // SAFETY: as above; the reply is left in the bundle's
                // argument buffer.
                let __reply = #v::bundle::host_fn_sync(&__args, |ptr, len| unsafe { __import(ptr, len) });
                let mut __input: &[u8] = &__reply;
                <#ret as #v::RemoteValue>::decode(&mut __input).unwrap_or_else(|e| panic!(
                    "host_fn `{}`: the reply does not decode ({}) — the load-time schema check should have refused this bundle",
                    stringify!(#name), e
                ))
            }
        }
    };

    Ok(quote! {
        ::runtime_vocabulary::__remote_guest_split! {
            bundle: { #stub }
            app: {
                #(#attrs)*
                #vis #sig #block

                // The allowlist record, only where remote components are
                // hosted (an app without them gets just the function).
                ::runtime_vocabulary::__remote_enabled! {
                    #[doc(hidden)]
                    #[allow(non_snake_case)]
                    #call_fn

                    #[doc(hidden)]
                    #[allow(non_snake_case)]
                    fn #export_ident() -> #v::HostFnDef {
                        #v::HostFnDef {
                            path: concat!(module_path!(), "::", stringify!(#name)),
                            schema: #schema,
                            kind: #kind,
                        }
                    }

                    /// The allowlist record for this host function: list it
                    /// in `install_with` to let remote code call it.
                    #[allow(non_snake_case)]
                    #vis mod #name {
                        pub fn export() -> ::runtime_vocabulary::remote::HostFnDef {
                            super::#export_ident()
                        }
                    }
                }
            }
        }
    })
}

/// 64-bit FNV-1a over `parts`, each prefixed with its length.
fn fnv1a(parts: &[String]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    for p in parts {
        eat(&(p.len() as u64).to_le_bytes());
        eat(p.as_bytes());
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The schema is a wire contract between separately built binaries:
    /// pinned, so a change to how it is computed is a deliberate one.
    #[test]
    fn the_schema_hash_is_pinned() {
        assert_eq!(fnv1a(&["u32".into(), "bool".into(), "sync".into()]), 0x4668_ca41_4944_a897);
        assert_ne!(fnv1a(&["ab".into(), "c".into()]), fnv1a(&["a".into(), "bc".into()]));
    }

    #[test]
    fn a_host_fn_splits_by_build_through_the_vocabulary() {
        let f: ItemFn = syn::parse_quote! { pub fn set_scheme(dark: bool) -> u32 { 1 } };
        let out = expand(f).unwrap().to_string();
        assert!(out.contains("__remote_guest_split"), "{out}");
        assert!(out.contains("__remote_enabled"), "{out}");
        assert!(!out.contains("cfg ("), "no raw cfg in the defining crate: {out}");
        assert!(out.contains("RemoteValue"), "{out}");
    }

    #[test]
    fn generic_host_fns_are_rejected() {
        let f: ItemFn = syn::parse_quote! { fn g<T>(t: T) {} };
        assert!(expand(f).is_err());
    }
}
