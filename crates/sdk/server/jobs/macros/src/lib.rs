//! `#[job]` — turn an `async fn` into a queue-backed background job.
//!
//! Modeled on `server`'s `#[server]`. For `#[job] async fn send_email(to:
//! String, db: State<Db>) -> Result<(), JobError> { … }` the macro emits a
//! module named after the fn:
//!
//! ```ignore
//! pub mod send_email {
//!     // Typed enqueue (compiles in every build). WIRE args only.
//!     pub fn enqueue(to: String) -> ::jobs::Enqueue { … }
//!
//!     // The body + handler registration (server / worker build only, gated on
//!     // the invoking crate's `server` feature — the same one `#[server]` uses).
//!     #[cfg(feature = "server")]
//!     pub async fn run(to: String, db: State<Db>) -> Result<(), JobError> { … }
//!     #[cfg(feature = "server")]
//!     inventory::submit! { JobEntry { name: "send_email", handler: … } }
//! }
//! ```
//!
//! Parameter classification mirrors `#[server]`: a param is an injected
//! extractor if it carries `#[ctx]` or its type's final segment is one of
//! `State` / `Headers` / `Extension` / `Auth` / `Cookies`; otherwise it is a
//! wire argument (serialized into the payload, present on `enqueue`). Job
//! handlers run outside any request, so only global extractors like `State<T>`
//! resolve meaningfully — request-scoped ones (`Headers`, `Cookies`, …) will
//! error at run time against the empty worker context.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{
    parse::Parser, parse_macro_input, punctuated::Punctuated, Attribute, FnArg, ItemFn, Meta, Pat,
    PatType, Token, Type,
};

/// Attribute options: `#[job(name = "…", queue = "…")]`.
#[derive(Default)]
struct JobAttr {
    name: Option<String>,
    queue: Option<String>,
}

fn parse_attr(attr: TokenStream) -> syn::Result<JobAttr> {
    let mut out = JobAttr::default();
    if attr.is_empty() {
        return Ok(out);
    }
    let parser = Punctuated::<Meta, Token![,]>::parse_terminated;
    let metas = parser.parse(attr)?;
    for meta in metas {
        if let Meta::NameValue(nv) = meta {
            let key = nv.path.get_ident().map(|i| i.to_string()).unwrap_or_default();
            let val = match &nv.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => s.value(),
                _ => {
                    return Err(syn::Error::new_spanned(
                        &nv.value,
                        "expected a string literal",
                    ))
                }
            };
            match key.as_str() {
                "name" => out.name = Some(val),
                "queue" => out.queue = Some(val),
                other => {
                    return Err(syn::Error::new_spanned(
                        &nv.path,
                        format!("unknown #[job] option `{other}` (expected `name` or `queue`)"),
                    ))
                }
            }
        } else {
            return Err(syn::Error::new_spanned(meta, "expected `key = \"value\"`"));
        }
    }
    Ok(out)
}

fn has_ctx_attr(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("ctx"))
}

/// The reserved extractor wrapper types — a param of one of these types is
/// injected, not sent over the wire. Mirrors `server`'s list.
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

/// Drop a `#[ctx]` attribute from a param (the emitted `run` fn shouldn't carry it).
fn without_ctx_attr(mut pt: PatType) -> PatType {
    pt.attrs.retain(|a| !a.path().is_ident("ctx"));
    pt
}

#[proc_macro_attribute]
pub fn job(attr: TokenStream, item: TokenStream) -> TokenStream {
    let attr = match parse_attr(attr) {
        Ok(a) => a,
        Err(e) => return e.to_compile_error().into(),
    };
    let func = parse_macro_input!(item as ItemFn);
    match expand(attr, func) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(attr: JobAttr, func: ItemFn) -> syn::Result<TokenStream2> {
    let ident = func.sig.ident.clone();
    let vis = func.vis.clone();
    let body = func.block.clone();
    let output = func.sig.output.clone();
    let job_name = attr.name.unwrap_or_else(|| ident.to_string());

    if func.sig.asyncness.is_none() {
        return Err(syn::Error::new_spanned(
            &func.sig.fn_token,
            "#[job] requires an `async fn`",
        ));
    }
    if let Some(recv) = func.sig.receiver() {
        return Err(syn::Error::new_spanned(
            recv,
            "#[job] cannot be applied to methods",
        ));
    }

    // Classify params into wire args vs injected extractors.
    let mut run_inputs: Vec<PatType> = Vec::new(); // all params, for `run`
    let mut wire_inputs: Vec<PatType> = Vec::new(); // wire params, for `enqueue`
    let mut wire_pats: Vec<Pat> = Vec::new();
    let mut wire_tys: Vec<Type> = Vec::new();
    let mut wire_binds: Vec<proc_macro2::Ident> = Vec::new();
    let mut ctx_tys: Vec<Type> = Vec::new();
    let mut ctx_binds: Vec<proc_macro2::Ident> = Vec::new();
    let mut call_exprs: Vec<proc_macro2::Ident> = Vec::new();

    for input in &func.sig.inputs {
        let FnArg::Typed(pt) = input else {
            continue; // receiver already rejected
        };
        let is_ctx = has_ctx_attr(&pt.attrs) || is_reserved_extractor(&pt.ty);
        run_inputs.push(without_ctx_attr(pt.clone()));
        if is_ctx {
            let bind = format_ident!("__c{}", ctx_binds.len());
            ctx_tys.push((*pt.ty).clone());
            ctx_binds.push(bind.clone());
            call_exprs.push(bind);
        } else {
            let bind = format_ident!("__w{}", wire_binds.len());
            wire_inputs.push(without_ctx_attr(pt.clone()));
            wire_pats.push((*pt.pat).clone());
            wire_tys.push((*pt.ty).clone());
            wire_binds.push(bind.clone());
            call_exprs.push(bind);
        }
    }

    // enqueue(...) → Enqueue, applying the default queue if the attr set one.
    let default_queue = match attr.queue {
        Some(q) => quote! { .queue(#q) },
        None => quote! {},
    };
    let enqueue_fn = quote! {
        // Always `pub` *within* the module — the module itself carries the
        // job's visibility, so a private `#[job]` stays private to its parent
        // while remaining callable as `name::enqueue(..)` from there.
        pub fn enqueue(#(#wire_inputs),*) -> ::jobs::Enqueue {
            ::jobs::Enqueue::new(
                #job_name,
                ::jobs::__private::encode_payload(&( #( #wire_pats, )* )),
            )
            #default_queue
        }
    };

    // Decode the payload tuple back into per-arg bindings inside the handler.
    let decode = if wire_tys.is_empty() {
        quote! { let _: () = ::jobs::__private::decode_payload(&__payload)?; }
    } else {
        quote! {
            let ( #( #wire_binds, )* ): ( #( #wire_tys, )* ) =
                ::jobs::__private::decode_payload(&__payload)?;
        }
    };

    // Resolve injected extractors against an empty (non-request) context.
    let ctx_resolution = if ctx_binds.is_empty() {
        quote! {}
    } else {
        quote! {
            let __ctx = ::server::Context::empty();
            #(
                let #ctx_binds = <#ctx_tys as ::server::FromContext>::from_context(&__ctx)
                    .await
                    .map_err(|__e| ::jobs::JobError::new(__e))?;
            )*
        }
    };

    let run_fn = quote! {
        #[cfg(feature = "server")]
        pub async fn run(#(#run_inputs),*) #output #body
    };

    let register = quote! {
        #[cfg(feature = "server")]
        ::jobs::__private::inventory::submit! {
            ::jobs::__private::JobEntry {
                name: #job_name,
                handler: |__payload| ::std::boxed::Box::pin(async move {
                    #decode
                    #ctx_resolution
                    run( #( #call_exprs ),* ).await
                }),
            }
        }
    };

    Ok(quote! {
        #[allow(non_snake_case, non_camel_case_types, dead_code)]
        #vis mod #ident {
            #[allow(unused_imports)]
            use super::*;

            #enqueue_fn
            #run_fn
            #register
        }
    })
}
