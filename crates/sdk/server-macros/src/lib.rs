//! Proc-macro half of the `server` SDK.
//!
//! Exposes `#[server]`, an attribute macro that turns an `async fn`
//! into two cfg-gated halves:
//!
//! - **Server build** (`feature = "server"` on the host crate): the
//!   original function body is preserved, plus an `inventory::submit!`
//!   that registers a handler with the runtime's dispatch table. The
//!   handler decodes the args tuple, awaits the function, and encodes
//!   the `Result` for the wire.
//!
//! - **Client build** (default features): the body is replaced with a
//!   call to `server::__private::call`, which serializes the args and
//!   POSTs them to the configured server.
//!
//! The two halves share the same signature, so call sites compile
//! identically on either side.
//!
//! Attribute arguments:
//! - `path = "..."` — override the wire path (default: the function
//!   name). Path is what appears after `/_srv/` in the URL.
//!
//! ```ignore
//! #[server]
//! async fn add(a: i32, b: i32) -> Result<i32, ServerError> {
//!     Ok(a + b)
//! }
//!
//! #[server(path = "v1/echo")]
//! async fn echo(s: String) -> Result<String, ServerError> { Ok(s) }
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, FnArg, ItemFn, Pat, PatType, ReturnType, Type};

/// Parses `#[server(path = "...")]` attribute arguments.
struct ServerAttr {
    path: Option<String>,
}

impl syn::parse::Parse for ServerAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut path = None;
        if input.is_empty() {
            return Ok(Self { path });
        }
        // Comma-separated key = "value" pairs. Only `path` is supported.
        let pairs =
            syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated(
                input,
            )?;
        for pair in pairs {
            let key = pair
                .path
                .get_ident()
                .map(|i| i.to_string())
                .unwrap_or_default();
            match key.as_str() {
                "path" => {
                    let syn::Expr::Lit(lit) = &pair.value else {
                        return Err(syn::Error::new_spanned(
                            &pair.value,
                            "expected string literal",
                        ));
                    };
                    let syn::Lit::Str(s) = &lit.lit else {
                        return Err(syn::Error::new_spanned(
                            &pair.value,
                            "expected string literal",
                        ));
                    };
                    path = Some(s.value());
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        &pair.path,
                        format!("unknown attribute `{other}`; supported: `path`"),
                    ));
                }
            }
        }
        Ok(Self { path })
    }
}

#[proc_macro_attribute]
pub fn server(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = parse_macro_input!(attr as ServerAttr);
    let func = parse_macro_input!(item as ItemFn);

    match expand(attr, func) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(attr: ServerAttr, func: ItemFn) -> syn::Result<TokenStream2> {
    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            func.sig.fn_token,
            "#[server] requires an async function",
        ));
    }

    let vis = &func.vis;
    let attrs = &func.attrs;
    let sig = &func.sig;
    let ident = &sig.ident;
    let inputs = &sig.inputs;
    let output = &sig.output;
    let body = &func.block;

    let wire_path = attr.path.unwrap_or_else(|| ident.to_string());

    // Pull out arg pattern (binding) + arg type for each parameter.
    // Receivers (`self`, `&self`) are rejected — server fns are
    // free functions only.
    let mut arg_pats: Vec<&Pat> = Vec::new();
    let mut arg_tys: Vec<&Type> = Vec::new();
    for input in inputs {
        match input {
            FnArg::Receiver(r) => {
                return Err(syn::Error::new_spanned(
                    r,
                    "#[server] functions cannot have a `self` receiver",
                ));
            }
            FnArg::Typed(PatType { pat, ty, .. }) => {
                arg_pats.push(pat.as_ref());
                arg_tys.push(ty.as_ref());
            }
        }
    }

    // The return type must be explicit — we need it for the client
    // stub's deserialization target. `-> ()` is technically a valid
    // server-fn return but doesn't make sense and would silently
    // succeed via Result<(), _>; punt and require the user to spell
    // it out.
    let ret_ty: &Type = match output {
        ReturnType::Type(_, ty) => ty.as_ref(),
        ReturnType::Default => {
            return Err(syn::Error::new_spanned(
                &sig.ident,
                "#[server] functions must declare a return type (e.g. `-> Result<T, ServerError>`)",
            ));
        }
    };

    // Fresh ident for the handler module, avoids colliding with any
    // user-defined item of the same name.
    let handler_mod = format_ident!("__server_fn_{}", ident);

    // -----------------------------------------------------------------
    // Server side (feature = "server"): original body + registration.
    // -----------------------------------------------------------------
    let server_half = {
        // Bind each tuple element to its declared name on entry to the
        // handler, then call the function. Generated names mirror the
        // user-declared bindings so error messages stay readable.
        let bind_idents: Vec<_> = (0..arg_pats.len())
            .map(|i| format_ident!("__arg{}", i))
            .collect();
        let call_args = bind_idents.iter();
        quote! {
            #(#attrs)*
            #vis async fn #ident(#inputs) #output #body

            // Hide the registration behind a private module so the
            // submitted item doesn't clutter the parent scope and the
            // helper types it references are namespaced.
            #[doc(hidden)]
            mod #handler_mod {
                use super::*;
                ::server::__private::inventory::submit! {
                    ::server::__private::ServerFnEntry {
                        path: #wire_path,
                        handler: |body_bytes| ::std::boxed::Box::pin(async move {
                            let ( #( #bind_idents, )* ): ( #( #arg_tys, )* ) =
                                ::server::__private::decode_args(&body_bytes)?;
                            let result: #ret_ty = super::#ident( #( #call_args ),* ).await;
                            ::server::__private::encode_result(&result)
                        }),
                    }
                }
            }
        }
    };

    // -----------------------------------------------------------------
    // Client side (no `server` feature): args → POST → result.
    // -----------------------------------------------------------------
    let client_half = quote! {
        #(#attrs)*
        #vis async fn #ident(#inputs) #output {
            let __args: ( #( #arg_tys, )* ) = ( #( #arg_pats, )* );
            ::server::__private::call::<( #( #arg_tys, )* ), #ret_ty>(
                #wire_path,
                &__args,
            ).await
        }
    };

    Ok(quote! {
        #[cfg(feature = "server")]
        #server_half

        #[cfg(not(feature = "server"))]
        #client_half
    })
}
