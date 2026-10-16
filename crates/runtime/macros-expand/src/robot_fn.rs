//! `#[robot_fn]` — a function a test can call by name over the robot
//! bridge (`runtime_vocabulary::robot_fn`).
//!
//! The function is emitted unchanged. Beside it, `<name>::robot()` returns
//! its `RobotFunction` record — name, doc comment, parameter names and
//! types, return type, whether it is async — for the app or SDK to
//! register at boot:
//!
//! ```ignore
//! /// Move every location watcher to the given place.
//! #[robot_fn(name = "location.set")]
//! pub fn set_position(latitude: f64, longitude: f64) { … }
//!
//! runtime_vocabulary::robot_fn::register(set_position::robot());
//! ```
//!
//! The record's adapter decodes each argument by name from the call's JSON
//! object and encodes the result; a `Result` return fails the call with
//! its `Err`'s `Display`. It is built only in a robot build
//! (`robot_select!`), so a release build never instantiates the serde
//! code.
//!
//! The module shares the function's name in the type namespace — the
//! `#[host_fn]` convention (`<name>::export()`) — so one `use` brings
//! both. It also means the attribute goes on a module-level function: a
//! method has no module to put it beside.

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, ToTokens};
use syn::{FnArg, ItemFn, Pat, ReturnType, Type};

/// `#[robot_fn]`'s arguments: an optional `name = "…"`.
fn parse_name(attr: TokenStream2) -> syn::Result<Option<String>> {
    if attr.is_empty() {
        return Ok(None);
    }
    let meta: syn::MetaNameValue = syn::parse2(attr)?;
    if !meta.path.is_ident("name") {
        return Err(syn::Error::new_spanned(&meta.path, "#[robot_fn] takes only `name = \"…\"`"));
    }
    match &meta.value {
        syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) if !s.value().is_empty() => Ok(Some(s.value())),
        other => Err(syn::Error::new_spanned(other, "#[robot_fn]: `name` must be a non-empty string literal")),
    }
}

/// The function's `///` comment, one line per attribute, the single space
/// rustdoc puts after `///` removed.
fn doc_of(func: &ItemFn) -> String {
    let mut lines = Vec::new();
    for attr in &func.attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                let line = s.value();
                lines.push(line.strip_prefix(' ').map(str::to_string).unwrap_or(line));
            }
        }
    }
    lines.join("\n").trim().to_string()
}

/// A type as source text: `Option<f64>`, not `Option < f64 >`.
fn type_text(ty: &impl ToTokens) -> String {
    let raw = ty.to_token_stream().to_string();
    let mut out = String::with_capacity(raw.len());
    let mut space_ok = false;
    for part in raw.split(' ').filter(|p| !p.is_empty()) {
        let glue_before = matches!(part, "<" | ">" | "::" | "," | ";" | ")" | "]");
        if space_ok && !glue_before {
            out.push(' ');
        }
        out.push_str(part);
        // No space after an opener or a path separator; one after `,`/`;`
        // and between words.
        space_ok = !matches!(part, "<" | "::" | "(" | "[" | "&");
    }
    out
}

/// Whether `ty` is spelled `Result<…>` (any path ending in `Result`).
fn is_result(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Result"))
}

pub fn expand(attr: TokenStream2, func: ItemFn) -> syn::Result<TokenStream2> {
    let explicit_name = parse_name(attr)?;
    let sig = &func.sig;
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &sig.generics,
            "#[robot_fn] can't be generic: a test calls it with JSON, which names no types",
        ));
    }
    if let Some(u) = &sig.unsafety {
        return Err(syn::Error::new_spanned(u, "#[robot_fn] can't be unsafe"));
    }
    if let Some(v) = &sig.variadic {
        return Err(syn::Error::new_spanned(v, "#[robot_fn] can't be variadic"));
    }

    let ident = &sig.ident;
    let vis = &func.vis;
    let name = explicit_name.unwrap_or_else(|| ident.to_string());
    let doc = doc_of(&func);
    let is_async = sig.asyncness.is_some();

    let mut arg_names = Vec::new();
    let mut arg_idents = Vec::new();
    let mut arg_types = Vec::new();
    for input in &sig.inputs {
        let pt = match input {
            FnArg::Receiver(r) => {
                return Err(syn::Error::new_spanned(
                    r,
                    "#[robot_fn] goes on a free function: a method has no module to put `<name>::robot()` beside",
                ))
            }
            FnArg::Typed(pt) => pt,
        };
        let Pat::Ident(pi) = &*pt.pat else {
            return Err(syn::Error::new_spanned(&pt.pat, "#[robot_fn]: each parameter needs a plain name — a test passes arguments by name"));
        };
        if let Type::Reference(r) = &*pt.ty {
            return Err(syn::Error::new_spanned(r, "#[robot_fn]: take an owned value — the argument is decoded from JSON for the call"));
        }
        arg_names.push(pi.ident.to_string());
        arg_idents.push(format_ident!("{}", pi.ident));
        arg_types.push((*pt.ty).clone());
    }
    let arg_type_texts: Vec<String> = arg_types.iter().map(type_text).collect();

    let (returns, encode): (String, TokenStream2) = match &sig.output {
        ReturnType::Default => ("()".into(), quote!(::runtime_vocabulary::robot_fn::__ok_json)),
        ReturnType::Type(_, ty) if is_result(ty) => (type_text(ty), quote!(::runtime_vocabulary::robot_fn::__result_json)),
        ReturnType::Type(_, ty) => (type_text(ty), quote!(::runtime_vocabulary::robot_fn::__ok_json)),
    };

    let rf = quote!(::runtime_vocabulary::robot_fn);
    let json = quote!(::runtime_vocabulary::glue::__serde_json);
    let record_ident = format_ident!("__robot_fn_{}", ident);
    let call = quote!(#ident(#(#arg_idents),*));
    let reply = if is_async {
        quote! {
            #rf::RobotReply::Pending(::std::boxed::Box::pin(async move { #encode(#call.await) }))
        }
    } else {
        quote! { #rf::RobotReply::Ready(#encode(#call)?) }
    };

    Ok(quote! {
        #func

        #[doc(hidden)]
        #[allow(non_snake_case)]
        fn #record_ident() -> #rf::RobotFunction {
            #rf::RobotFunction {
                name: #name,
                doc: #doc,
                args: &[#(#rf::RobotArg { name: #arg_names, ty: #arg_type_texts }),*],
                returns: #returns,
                is_async: #is_async,
                call: ::runtime_vocabulary::robot_select! {
                    on: {
                        ::core::option::Option::Some(::std::rc::Rc::new(
                            |__args: &#json::Value| -> ::core::result::Result<#rf::RobotReply, ::std::string::String> {
                                #(let #arg_idents: #arg_types = #rf::__arg(__args, #name, #arg_names)?;)*
                                ::core::result::Result::Ok(#reply)
                            },
                        ))
                    }
                    off: { ::core::option::Option::None }
                },
            }
        }

        /// The robot record for this function: pass it to
        /// `runtime_vocabulary::robot_fn::register` at boot.
        #[allow(non_snake_case)]
        #vis mod #ident {
            pub fn robot() -> ::runtime_vocabulary::robot_fn::RobotFunction {
                super::#record_ident()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(attr: TokenStream2, item: TokenStream2) -> syn::Result<String> {
        expand(attr, syn::parse2(item).unwrap()).map(|t| t.to_string())
    }

    fn squash(s: &str) -> String {
        s.chars().filter(|c| !c.is_whitespace()).collect()
    }

    #[test]
    fn emits_the_function_its_record_and_the_named_module() {
        let out = run(
            quote!(name = "location.set"),
            quote! {
                /// Move every watcher.
                ///
                /// Second paragraph.
                pub fn set_position(latitude: f64, accuracy: Option<f64>) -> Result<(), String> { Ok(()) }
            },
        )
        .unwrap();
        let s = squash(&out);
        assert!(s.contains("pubfnset_position(latitude:f64,accuracy:Option<f64>)"), "function kept: {out}");
        assert!(s.contains("name:\"location.set\""), "{out}");
        assert!(s.contains(r#"doc:"Moveeverywatcher.\n\nSecondparagraph.""#), "doc lines joined: {out}");
        assert!(s.contains("RobotArg{name:\"latitude\",ty:\"f64\"}"), "{out}");
        assert!(s.contains("RobotArg{name:\"accuracy\",ty:\"Option<f64>\"}"), "{out}");
        assert!(s.contains("returns:\"Result<(),String>\""), "{out}");
        assert!(s.contains("is_async:false"), "{out}");
        assert!(s.contains("__result_json(set_position(latitude,accuracy))?"), "a Result return maps its Err: {out}");
        assert!(s.contains("pubmodset_position{pubfnrobot()"), "the module shares the fn's name and visibility: {out}");
        assert!(s.contains("::runtime_vocabulary::robot_select!{on:{"), "the adapter is robot-build only: {out}");
    }

    #[test]
    fn defaults_the_name_and_encodes_a_plain_return() {
        let s = squash(&run(quote!(), quote! { fn seed(count: u32) -> Vec<String> { vec![] } }).unwrap());
        assert!(s.contains("name:\"seed\""), "{s}");
        assert!(s.contains("returns:\"Vec<String>\""), "{s}");
        assert!(s.contains("__ok_json(seed(count))?"), "{s}");
        assert!(s.contains("modseed{"), "private fn, private module: {s}");
    }

    #[test]
    fn an_async_function_returns_its_future() {
        let s = squash(&run(quote!(), quote! { async fn fetch(id: u64) -> Result<u64, String> { Ok(id) } }).unwrap());
        assert!(s.contains("is_async:true"), "{s}");
        assert!(s.contains("RobotReply::Pending(::std::boxed::Box::pin(asyncmove{::runtime_vocabulary::robot_fn::__result_json(fetch(id).await)}))"), "{s}");
    }

    #[test]
    fn a_unit_return_is_null() {
        let s = squash(&run(quote!(), quote! { fn reset() {} }).unwrap());
        assert!(s.contains("returns:\"()\""), "{s}");
        assert!(s.contains("args:&[]"), "{s}");
    }

    #[test]
    fn validations_reject_what_a_json_call_cannot_reach() {
        for (attr, item, needle) in [
            (quote!(), quote! { fn f<T>(x: T) {} }, "generic"),
            (quote!(), quote! { fn f(&self) {} }, "free function"),
            (quote!(), quote! { fn f(s: &str) {} }, "owned value"),
            (quote!(), quote! { fn f((a, b): (u8, u8)) {} }, "plain name"),
            (quote!(), quote! { unsafe fn f() {} }, "unsafe"),
            (quote!(label = "x"), quote! { fn f() {} }, "only `name"),
            (quote!(name = ""), quote! { fn f() {} }, "non-empty"),
        ] {
            let err = run(attr, item).unwrap_err().to_string();
            assert!(err.contains(needle), "expected {needle:?} in: {err}");
        }
    }

    #[test]
    fn type_text_reads_like_source() {
        let t: Type = syn::parse_quote!(std::collections::HashMap<String, Vec<(u8, f64)>>);
        assert_eq!(type_text(&t), "std::collections::HashMap<String, Vec<(u8, f64)>>");
        let t: Type = syn::parse_quote!([u8; 4]);
        assert_eq!(type_text(&t), "[u8; 4]");
    }
}
