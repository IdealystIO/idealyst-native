//! `#[host_fn]` — an app function a streamed bundle can call.
//!
//! The shape is `#[server]` pointed the other way: ONE definition, in a
//! crate both the app and the bundle depend on, compiles to the real
//! function in the app and to a stub in a bundle.
//!
//! Which side is decided by `cfg(idealyst_stream_guest)`, a rustc `--cfg`
//! the bundle build passes — deliberately NOT a cargo feature. Features
//! unify: one `cargo build` that includes both the app and a guest crate
//! would turn a `guest` feature on for the app too, and the app would
//! silently get stubs instead of its own functions. A `--cfg` belongs to a
//! build, and a bundle is always its own build.
//!
//! App side (no cfg):
//! - the function, unchanged;
//! - `<name>::export()` → a `stream_abi::host_fn::HostFnDef` the app hands
//!   to `HostExports::host_fn` — the explicit allowlist of what bundles may
//!   call.
//!
//! Bundle side (`idealyst_stream_guest`):
//! - a sync fn keeps its signature; its body becomes one wasm import call;
//! - an `async fn` becomes a plain fn returning
//!   `stream_guest::HostCall<Output>`, driven by `stream_guest::spawn_then`
//!   — the same call shape as native `spawn_then(future, then)`.
//!
//! Every stub links a wasm import named `<module_path>::<fn>#<schema>`, so
//! the bundle's import section lists exactly the host functions it uses.
//! `<schema>` is a fingerprint of the arg types, return type and
//! asyncness (`#[server]`'s scheme); the host compares it at load.
//!
//! The defining crate depends on `stream-abi` always and `stream-guest` in
//! bundle builds (`[target.'cfg(idealyst_stream_guest)'.dependencies]`).

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, FnArg, ItemFn, Pat, ReturnType, Type};

#[proc_macro_attribute]
pub fn host_fn(attr: TokenStream, item: TokenStream) -> TokenStream {
    if !attr.is_empty() {
        return syn::Error::new(proc_macro2::Span::call_site(), "#[host_fn] takes no arguments")
            .to_compile_error()
            .into();
    }
    let func = parse_macro_input!(item as ItemFn);
    match expand(func) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(func: ItemFn) -> syn::Result<TokenStream2> {
    let sig = &func.sig;
    let name = &sig.ident;
    let vis = &func.vis;
    let is_async = sig.asyncness.is_some();
    if !sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(&sig.generics, "#[host_fn] cannot be generic: the signature is a wire contract"));
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

    // Same fingerprint scheme as `#[server]`: the type spellings, hashed with
    // `DefaultHasher`'s fixed seed so it is stable across compilations.
    let schema: u64 = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for ty in &arg_types {
            quote!(#ty).to_string().hash(&mut h);
        }
        ret.to_string().hash(&mut h);
        is_async.hash(&mut h);
        h.finish()
    };
    let schema_hex = format!("{schema:016x}");

    let call_ident = format_ident!("__host_fn_call_{}", name);
    let export_ident = format_ident!("__host_fn_export_{}", name);
    let decode_args = quote! {
        let mut __input: &[u8] = &__args;
        #(
            let #arg_names: #arg_types = <#arg_types as ::stream_abi::Wire>::decode(&mut __input)
                .unwrap_or_else(|| panic!(
                    "host_fn `{}`: argument `{}` does not decode — the load-time schema check should have refused this bundle",
                    stringify!(#name), stringify!(#arg_names)
                ));
        )*
    };

    let (call_fn, kind) = if is_async {
        (
            quote! {
                fn #call_ident(__args: ::std::vec::Vec<u8>)
                    -> ::std::pin::Pin<::std::boxed::Box<dyn ::std::future::Future<Output = ::std::vec::Vec<u8>>>>
                {
                    #decode_args
                    ::std::boxed::Box::pin(async move {
                        ::stream_abi::Wire::to_bytes(&#name(#(#arg_names),*).await)
                    })
                }
            },
            quote!(::stream_abi::host_fn::HostFnKind::Async(#call_ident)),
        )
    } else {
        (
            quote! {
                fn #call_ident(__args: &[u8]) -> ::std::vec::Vec<u8> {
                    #decode_args
                    ::stream_abi::Wire::to_bytes(&#name(#(#arg_names),*))
                }
            },
            quote!(::stream_abi::host_fn::HostFnKind::Sync(#call_ident)),
        )
    };

    let attrs = &func.attrs;
    let block = &func.block;
    let inputs = &sig.inputs;

    let guest_stub = if is_async {
        quote! {
            #(#attrs)*
            #vis fn #name(#inputs) -> ::stream_guest::HostCall<#ret> {
                #[link(wasm_import_module = "idealyst_host_fn")]
                extern "C" {
                    #[link_name = concat!(module_path!(), "::", stringify!(#name), "#", #schema_hex)]
                    fn __import(args_ptr: *const u8, args_len: u32, then: u32);
                }
                let mut __args = ::std::vec::Vec::new();
                #( ::stream_guest::Wire::encode(&#arg_names, &mut __args); )*
                // SAFETY: the host reads `args_len` bytes at `args_ptr` during
                // the call and copies them out.
                ::stream_guest::HostCall::new(__args, |ptr, len, then| unsafe { __import(ptr, len, then) })
            }
        }
    } else {
        quote! {
            #(#attrs)*
            #vis fn #name(#inputs) -> #ret {
                #[link(wasm_import_module = "idealyst_host_fn")]
                extern "C" {
                    #[link_name = concat!(module_path!(), "::", stringify!(#name), "#", #schema_hex)]
                    fn __import(args_ptr: *const u8, args_len: u32) -> u64;
                }
                let mut __args = ::std::vec::Vec::new();
                #( ::stream_guest::Wire::encode(&#arg_names, &mut __args); )*
                // SAFETY: as above; the returned buffer comes from `stream_alloc`.
                let __packed = unsafe { __import(__args.as_ptr(), __args.len() as u32) };
                ::stream_guest::__rt::host_result::<#ret>(__packed, stringify!(#name))
            }
        }
    };

    Ok(quote! {
        // ---- app side -------------------------------------------------------
        #[cfg(not(idealyst_stream_guest))]
        #(#attrs)*
        #vis #sig #block

        #[cfg(not(idealyst_stream_guest))]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        #call_fn

        #[cfg(not(idealyst_stream_guest))]
        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #export_ident() -> ::stream_abi::host_fn::HostFnDef {
            ::stream_abi::host_fn::HostFnDef {
                path: concat!(module_path!(), "::", stringify!(#name)),
                schema: #schema,
                kind: #kind,
            }
        }

        /// The export record for this host function — pass to
        /// `HostExports::host_fn` to let bundles call it.
        #[cfg(not(idealyst_stream_guest))]
        #vis mod #name {
            pub fn export() -> ::stream_abi::host_fn::HostFnDef {
                super::#export_ident()
            }
        }

        // ---- bundle side ----------------------------------------------------
        #[cfg(idealyst_stream_guest)]
        #guest_stub
    })
}
