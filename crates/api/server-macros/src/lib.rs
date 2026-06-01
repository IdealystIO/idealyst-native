//! Proc-macro half of the `server` SDK.
//!
//! Exposes `#[server]`, an attribute macro that turns an `async fn`
//! into two cfg-gated halves:
//!
//! - **Server build** (`feature = "server"` on the host crate): the
//!   original function body is preserved, plus an `inventory::submit!`
//!   that registers a handler with the runtime's dispatch table. The
//!   handler decodes the wire args, resolves any injected extractor
//!   params from the request context, awaits the function, and encodes
//!   the `Result` for the wire.
//!
//! - **Client build** (default features): the body is replaced with a
//!   call to `server::__private::call`, which serializes the wire args
//!   and POSTs them to the configured server.
//!
//! ## Parameters: wire args vs. injected extractors
//!
//! Each parameter is classified as one of:
//!
//! - **Wire arg** — serialized into the request body and present in the
//!   client stub's signature. The default for any parameter.
//! - **Injected extractor** — resolved server-side from the request
//!   [`Context`] via `FromContext`, and *omitted* from the client stub.
//!   A parameter is an extractor if it is annotated `#[ctx]` **or** its
//!   type is one of the reserved wrapper names (`State`, `Headers`,
//!   `Extension`, `Auth`, `Cookies`).
//!
//! Because a proc-macro sees syntax, not resolved trait impls, the
//! classification is syntactic: the reserved names cover the built-in
//! extractors with zero ceremony, and `#[ctx]` opts any other
//! `FromContext` type in. The client stub never resolves anything — it
//! just drops the extractor params — so the wire signature stays
//! `(wire args…) -> Ret` on both sides.
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
//! // `db`/`headers` are injected server-side; the client stub is
//! // `create_todo(input: NewTodo) -> Result<Todo, _>`.
//! #[server]
//! async fn create_todo(
//!     input: NewTodo,
//!     db: State<Db>,
//!     headers: Headers,
//! ) -> Result<Todo, ServerError<E>> { ... }
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{parse_macro_input, Attribute, FnArg, Ident, ItemFn, Pat, PatType, ReturnType, Type};

/// Parses `#[server(path = "...", strict_version)]` attribute arguments.
struct ServerAttr {
    path: Option<String>,
    /// When set, the server rejects any client whose wire schema hash
    /// differs from this fn's — up front, before decoding — with an
    /// `IncompatibleVersion` (426). For irreversible / money-movement
    /// endpoints where "it happened to deserialize" is not good enough.
    strict: bool,
}

impl syn::parse::Parse for ServerAttr {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let mut path = None;
        let mut strict = false;
        if input.is_empty() {
            return Ok(Self { path, strict });
        }
        // Comma-separated `Meta`: `path = "..."` (name-value) and the
        // bare flag `strict_version` (path).
        let metas =
            syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated(input)?;
        for meta in metas {
            match meta {
                syn::Meta::NameValue(nv) if nv.path.is_ident("path") => {
                    let syn::Expr::Lit(lit) = &nv.value else {
                        return Err(syn::Error::new_spanned(&nv.value, "expected string literal"));
                    };
                    let syn::Lit::Str(s) = &lit.lit else {
                        return Err(syn::Error::new_spanned(&nv.value, "expected string literal"));
                    };
                    path = Some(s.value());
                }
                syn::Meta::Path(p) if p.is_ident("strict_version") => {
                    strict = true;
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        other,
                        "unknown attribute; supported: `path = \"...\"`, `strict_version`",
                    ));
                }
            }
        }
        Ok(Self { path, strict })
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

/// `true` if the parameter is annotated with the `#[ctx]` helper.
fn has_ctx_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("ctx"))
}

/// `true` if the parameter's type names a reserved built-in extractor
/// wrapper (`State`, `Headers`, `Extension`) — recognised by its final
/// path segment so both `State<T>` and `server::State<T>` match. These
/// are injected without needing `#[ctx]`.
fn is_reserved_extractor(ty: &Type) -> bool {
    if let Type::Path(tp) = ty {
        if let Some(seg) = tp.path.segments.last() {
            return matches!(
                seg.ident.to_string().as_str(),
                "State" | "Headers" | "Extension" | "Auth" | "Cookies"
            );
        }
    }
    false
}

/// Strip the `#[ctx]` helper from a parameter's attributes (it isn't a
/// real attribute and must not survive into the emitted function).
fn without_ctx_attr(pt: &PatType) -> FnArg {
    let attrs: Vec<Attribute> = pt
        .attrs
        .iter()
        .filter(|a| !a.path().is_ident("ctx"))
        .cloned()
        .collect();
    FnArg::Typed(PatType {
        attrs,
        pat: pt.pat.clone(),
        colon_token: pt.colon_token,
        ty: pt.ty.clone(),
    })
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

    // Classify each parameter into a wire arg or an injected extractor,
    // building the token fragments each emitted half needs. Receivers
    // (`self`) are rejected — server fns are free functions only.
    //
    // - `server_inputs`: every param (ctx attr stripped) → the server fn
    //   signature, which keeps all params since the body uses them.
    // - `wire_inputs` / `wire_pats` / `wire_tys` / `wire_binds`: wire
    //   params only → the client stub signature, the args tuple, and the
    //   server-side decode.
    // - `ctx_tys` / `ctx_binds`: extractor params → server-side
    //   `FromContext` resolution.
    // - `call_exprs`: every param's binding ident in declaration order →
    //   the positional call into the real fn.
    let mut server_inputs: Vec<FnArg> = Vec::new();
    let mut wire_inputs: Vec<FnArg> = Vec::new();
    let mut wire_pats: Vec<Pat> = Vec::new();
    let mut wire_tys: Vec<Type> = Vec::new();
    let mut wire_binds: Vec<Ident> = Vec::new();
    let mut ctx_tys: Vec<Type> = Vec::new();
    let mut ctx_binds: Vec<Ident> = Vec::new();
    let mut call_exprs: Vec<Ident> = Vec::new();

    for input in inputs {
        let pt = match input {
            FnArg::Receiver(r) => {
                return Err(syn::Error::new_spanned(
                    r,
                    "#[server] functions cannot have a `self` receiver",
                ));
            }
            FnArg::Typed(pt) => pt,
        };

        let is_ctx = has_ctx_attr(&pt.attrs) || is_reserved_extractor(&pt.ty);
        server_inputs.push(without_ctx_attr(pt));

        if is_ctx {
            let bind = format_ident!("__c{}", ctx_binds.len());
            ctx_tys.push((*pt.ty).clone());
            ctx_binds.push(bind.clone());
            call_exprs.push(bind);
        } else {
            let bind = format_ident!("__w{}", wire_binds.len());
            wire_inputs.push(without_ctx_attr(pt));
            wire_pats.push((*pt.pat).clone());
            wire_tys.push((*pt.ty).clone());
            wire_binds.push(bind.clone());
            call_exprs.push(bind);
        }
    }

    // The return type must be explicit — the client stub needs it as the
    // deserialization target.
    let ret_ty: &Type = match output {
        ReturnType::Type(_, ty) => ty.as_ref(),
        ReturnType::Default => {
            return Err(syn::Error::new_spanned(
                &sig.ident,
                "#[server] functions must declare a return type (e.g. `-> Result<T, ServerError>`)",
            ));
        }
    };

    // Wire schema hash: a structural fingerprint of the wire contract —
    // the serialized arg types + the return type. Computed at expansion
    // time from the type spelling and embedded as a const on BOTH sides.
    // Identical source → identical hash; a drifted arg/return type → a
    // different hash, which turns an otherwise-vague decode failure into
    // a precise `IncompatibleVersion`. Ctx (extractor) params are not on
    // the wire, so they don't participate. Uses `DefaultHasher`, whose
    // fixed seed makes the value stable across compilations.
    let schema_hash: u64 = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        for ty in &wire_tys {
            quote!(#ty).to_string().hash(&mut h);
        }
        quote!(#ret_ty).to_string().hash(&mut h);
        h.finish()
    };
    let strict = attr.strict;

    // Fresh ident for the handler module, avoids colliding with any
    // user-defined item of the same name.
    let handler_mod = format_ident!("__server_fn_{}", ident);

    // -----------------------------------------------------------------
    // Server side (feature = "server"): original body + registration.
    //
    // The `#[cfg]` attaches to a single item, so the fn and the
    // registration module are each gated independently (one outer
    // `#[cfg]` over both would leave the module compiled on the client,
    // where its handler can't satisfy `ServerFnEntry`'s `Send` bound).
    // -----------------------------------------------------------------
    let server_fn = quote! {
        #[cfg(feature = "server")]
        #(#attrs)*
        #vis async fn #ident(#(#server_inputs),*) #output #body
    };

    // Resolve each injected extractor from the current request context.
    // Skipped entirely when there are none, so the all-wire path emits
    // exactly what v0 did (no `current_context` call, no unused binding).
    let ctx_resolution = if ctx_binds.is_empty() {
        quote! {}
    } else {
        quote! {
            let __ctx = ::server::__private::current_context();
            #(
                let #ctx_binds = <#ctx_tys as ::server::FromContext>::from_context(&__ctx).await?;
            )*
        }
    };

    let server_register = quote! {
        #[cfg(feature = "server")]
        #[doc(hidden)]
        mod #handler_mod {
            use super::*;
            ::server::__private::inventory::submit! {
                ::server::__private::ServerFnEntry {
                    path: #wire_path,
                    schema: #schema_hash,
                    strict: #strict,
                    handler: |__body_bytes| ::std::boxed::Box::pin(async move {
                        let ( #( #wire_binds, )* ): ( #( #wire_tys, )* ) =
                            ::server::__private::decode_args(&__body_bytes)?;
                        #ctx_resolution
                        let __result: #ret_ty = super::#ident( #( #call_exprs ),* ).await;
                        ::server::__private::encode_result(&__result)
                    }),
                }
            }
        }
    };

    // -----------------------------------------------------------------
    // Client side (no `server` feature): wire args → POST → result.
    // Extractor params are dropped from the signature and the tuple.
    // -----------------------------------------------------------------
    let client_fn = quote! {
        #[cfg(not(feature = "server"))]
        #(#attrs)*
        #vis async fn #ident(#(#wire_inputs),*) #output {
            let __args: ( #( #wire_tys, )* ) = ( #( #wire_pats, )* );
            ::server::__private::call::<( #( #wire_tys, )* ), #ret_ty>(
                #wire_path,
                #schema_hash,
                &__args,
            ).await
        }
    };

    Ok(quote! {
        #server_fn
        #server_register
        #client_fn
    })
}
