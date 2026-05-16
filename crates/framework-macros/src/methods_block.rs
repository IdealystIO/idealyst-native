//! `methods! { ... }` block lifting inside `#[component]`.
//!
//! A component author can declare imperative methods exposed to their
//! parent by writing a `methods!` block at the top level of the
//! component's body:
//!
//! ```ignore
//! #[component]
//! pub fn counter(props: &CounterProps) -> Bindable<CounterHandle> {
//!     let value = props.value;
//!     methods! {
//!         fn reset(&self) { value.set(0); }
//!         fn bump_by(&self, n: i32) { value.update(|v| *v += n); }
//!     }
//!     view(children![/* ... */])
//! }
//! ```
//!
//! This module:
//!  - finds the `methods!` statement in the function body,
//!  - parses its inner `fn name(&self, args...) { body }` declarations,
//!  - generates a sibling `CounterHandle` struct with `Rc<dyn Fn(...)>`
//!    fields, a `Clone` impl, and `pub fn name(...)` accessors that
//!    invoke each closure,
//!  - replaces the in-body `methods!` statement with a `let __handle
//!    = CounterHandle { ... }` construction whose fields hold `move`
//!    closures wrapping each method body,
//!  - wraps the function's trailing expression in
//!    `Bindable::new(<tail>.into(), __handle)` so the implicit return
//!    matches the `Bindable<H>` return type.
//!
//! Author rules:
//!  - exactly zero or one `methods!` block per component,
//!  - methods take `&self` (cosmetic — captures come from the closure,
//!    not from struct fields) plus zero or more typed args,
//!  - method bodies return `()` only (v1 limitation).
//!
//! The handle's name is derived from the component's fn name by
//! converting `snake_case` to `PascalCase` and appending `Handle`
//! (`counter` → `CounterHandle`).

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{Block, FnArg, Ident, ItemFn, Pat, Stmt, Type};

/// Extract any `methods!` block from the function body. If present:
///  - synthesizes the handle struct + impls (returned in the
///    `TokenStream2`),
///  - replaces the in-body `methods!` statement with a handle
///    construction binding,
///  - wraps the tail expression with `Bindable::new(...)`.
///
/// If no `methods!` block is present, returns an empty `TokenStream2`
/// and leaves the function untouched.
pub(crate) fn extract_and_rewrite(item_fn: &mut ItemFn) -> syn::Result<TokenStream2> {
    let Some((idx, methods)) = find_and_parse_methods(item_fn)? else {
        return Ok(TokenStream2::new());
    };

    let handle_name = derive_handle_name(&item_fn.sig.ident);
    let fn_name = item_fn.sig.ident.clone();
    let extra = generate_handle_type(&handle_name, &methods);

    rewrite_body(item_fn, idx, &handle_name, &fn_name, &methods);

    Ok(extra)
}

/// Walks the function body for a `methods! { ... }` statement. Returns
/// (statement index, parsed methods) if found, or `None`. Errors if
/// more than one `methods!` block is present.
fn find_and_parse_methods(item_fn: &ItemFn) -> syn::Result<Option<(usize, Vec<MethodDef>)>> {
    let mut found: Option<(usize, &syn::StmtMacro)> = None;
    for (i, stmt) in item_fn.block.stmts.iter().enumerate() {
        if let Stmt::Macro(m) = stmt {
            if m.mac.path.is_ident("methods") {
                if found.is_some() {
                    return Err(syn::Error::new_spanned(
                        &m.mac.path,
                        "only one `methods!` block per component is allowed",
                    ));
                }
                found = Some((i, m));
            }
        }
    }
    let Some((idx, m)) = found else { return Ok(None) };
    let parsed: MethodsBody = syn::parse2(m.mac.tokens.clone())?;
    Ok(Some((idx, parsed.methods)))
}

/// Parsed contents of a `methods! { ... }` block.
struct MethodsBody {
    methods: Vec<MethodDef>,
}

impl Parse for MethodsBody {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut methods = Vec::new();
        while !input.is_empty() {
            methods.push(input.parse()?);
        }
        Ok(MethodsBody { methods })
    }
}

/// One method declaration inside `methods! { ... }`.
struct MethodDef {
    name: Ident,
    /// Args other than `&self`. Each is (binding, type).
    args: Vec<(Ident, Type)>,
    body: Block,
}

impl Parse for MethodDef {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let _fn_kw: syn::Token![fn] = input.parse()?;
        let name: Ident = input.parse()?;
        let args_content;
        syn::parenthesized!(args_content in input);
        // Use syn's FnArg parser so we get the same error messages as
        // a regular fn signature. The first arg must be `&self`.
        let raw_args: syn::punctuated::Punctuated<FnArg, syn::Token![,]> =
            args_content.parse_terminated(FnArg::parse, syn::Token![,])?;
        let mut iter = raw_args.into_iter();
        match iter.next() {
            Some(FnArg::Receiver(r)) => {
                if r.reference.is_none() || r.mutability.is_some() {
                    return Err(syn::Error::new_spanned(
                        r,
                        "methods! receivers must be `&self` (no `mut`, no owned `self`)",
                    ));
                }
            }
            other => {
                return Err(syn::Error::new_spanned(
                    other.map(|x| quote!(#x)).unwrap_or_else(|| quote!()),
                    "methods! functions must start with `&self`",
                ));
            }
        }
        let mut args = Vec::new();
        for a in iter {
            match a {
                FnArg::Typed(pt) => {
                    let ident = match &*pt.pat {
                        Pat::Ident(pi) => pi.ident.clone(),
                        _ => {
                            return Err(syn::Error::new_spanned(
                                &pt.pat,
                                "methods! arguments must use plain identifier patterns",
                            ));
                        }
                    };
                    args.push((ident, (*pt.ty).clone()));
                }
                FnArg::Receiver(r) => {
                    return Err(syn::Error::new_spanned(
                        r,
                        "only the first methods! argument may be `&self`",
                    ));
                }
            }
        }
        // No return type in v1 — bodies must return `()`. If the user
        // writes `-> T`, parse it and reject.
        if input.peek(syn::Token![->]) {
            let arrow: syn::Token![->] = input.parse()?;
            let _ty: Type = input.parse()?;
            return Err(syn::Error::new_spanned(
                arrow,
                "methods! return types are not supported yet; use `()`",
            ));
        }
        let body: Block = input.parse()?;
        Ok(MethodDef { name, args, body })
    }
}

/// `counter` → `CounterHandle`. snake_case to PascalCase + Handle.
fn derive_handle_name(fn_name: &Ident) -> Ident {
    let raw = fn_name.to_string();
    let mut pascal = String::with_capacity(raw.len());
    let mut next_upper = true;
    for c in raw.chars() {
        if c == '_' {
            next_upper = true;
        } else if next_upper {
            pascal.extend(c.to_uppercase());
            next_upper = false;
        } else {
            pascal.push(c);
        }
    }
    pascal.push_str("Handle");
    Ident::new(&pascal, fn_name.span())
}

/// Emits:
///   pub struct CounterHandle { __reset: Rc<dyn Fn()>, ... }
///   impl Clone for CounterHandle { fn clone(&self) -> Self { ... } }
///   impl CounterHandle { pub fn reset(&self) { (self.__reset)() } ... }
fn generate_handle_type(name: &Ident, methods: &[MethodDef]) -> TokenStream2 {
    let fields = methods.iter().map(|m| {
        let f = field_ident(&m.name);
        let arg_tys = m.args.iter().map(|(_, ty)| ty);
        quote! { #f: ::std::rc::Rc<dyn Fn(#(#arg_tys),*)> }
    });
    let clone_fields = methods.iter().map(|m| {
        let f = field_ident(&m.name);
        quote! { #f: ::std::clone::Clone::clone(&self.#f) }
    });
    let accessors = methods.iter().map(|m| {
        let method_name = &m.name;
        let f = field_ident(&m.name);
        let arg_names = m.args.iter().map(|(n, _)| n);
        let arg_names2 = m.args.iter().map(|(n, _)| n);
        let arg_tys = m.args.iter().map(|(_, ty)| ty);
        quote! {
            pub fn #method_name(&self, #(#arg_names: #arg_tys),*) {
                (self.#f)(#(#arg_names2),*);
            }
        }
    });

    quote! {
        pub struct #name {
            #(#fields,)*
        }

        impl ::std::clone::Clone for #name {
            fn clone(&self) -> Self {
                Self {
                    #(#clone_fields,)*
                }
            }
        }

        impl #name {
            #(#accessors)*
        }
    }
}

/// Mutates the function body in place:
///   1. Replaces the methods! statement at `idx` with a let-binding
///      that constructs the handle by wrapping each method body in
///      a `move` closure (so captures happen at the call site, not
///      inside an impl method).
///   2. Wraps the trailing tail expression with
///      `Bindable::new(<tail>.into(), __handle)`.
fn rewrite_body(
    item_fn: &mut ItemFn,
    idx: usize,
    handle_name: &Ident,
    fn_name: &Ident,
    methods: &[MethodDef],
) {
    // Build the handle construction block.
    let field_inits = methods.iter().map(|m| {
        let f = field_ident(&m.name);
        let body = &m.body;
        let arg_names = m.args.iter().map(|(n, _)| n);
        let arg_tys = m.args.iter().map(|(_, ty)| ty);
        let arg_tys2 = m.args.iter().map(|(_, ty)| ty);
        // The cast is necessary so the closure coerces to the trait
        // object the field expects — closure types are unique per
        // closure, but `Rc<dyn Fn(...)>` is a single concrete field
        // type.
        quote! {
            #f: {
                let __c = ::std::rc::Rc::new(move |#(#arg_names: #arg_tys),*| #body);
                __c as ::std::rc::Rc<dyn Fn(#(#arg_tys2),*)>
            }
        }
    });

    let handle_construction: Stmt = syn::parse_quote! {
        let __component_handle = #handle_name {
            #(#field_inits,)*
        };
    };

    // Replace the methods! macro stmt with the construction binding.
    item_fn.block.stmts[idx] = handle_construction;

    // Insert robot auto-registration directly after the handle binding.
    // Each `methods!` declaration becomes a JSON-callable adapter that
    // deserializes each argument by name, then forwards to the handle's
    // method. The whole block is `#[cfg]`-gated to the consuming crate's
    // `robot` feature so non-robot builds pay nothing.
    let component_name_str = fn_name.to_string();
    let method_entries = methods.iter().map(|m| {
        let method_name_str = m.name.to_string();
        let arg_names_str: Vec<String> = m.args.iter().map(|(n, _)| n.to_string()).collect();
        let arg_idents: Vec<&Ident> = m.args.iter().map(|(n, _)| n).collect();
        let arg_idents_for_call = arg_idents.clone();
        let arg_tys: Vec<&Type> = m.args.iter().map(|(_, ty)| ty).collect();
        let method_ident = &m.name;
        let arg_extractions = arg_idents.iter().zip(arg_tys.iter()).zip(arg_names_str.iter()).map(|((ident, ty), name)| {
            quote! {
                let #ident: #ty = ::framework_core::__serde_json::from_value(
                    __args.get(#name).cloned().unwrap_or(::framework_core::__serde_json::Value::Null),
                ).map_err(|e| ::std::format!("arg '{}': {}", #name, e))?;
            }
        });
        quote! {
            ::framework_core::robot::Method {
                name: #method_name_str,
                args: &[#(#arg_names_str),*],
                invoke: {
                    let __h = ::std::clone::Clone::clone(&__component_handle);
                    ::std::rc::Rc::new(move |__args: &::framework_core::__serde_json::Value| -> ::std::result::Result<(), ::std::string::String> {
                        #(#arg_extractions)*
                        __h.#method_ident(#(#arg_idents_for_call),*);
                        ::std::result::Result::Ok(())
                    })
                },
            }
        }
    });

    let registration_stmt: Stmt = syn::parse_quote! {
        #[cfg(feature = "robot")]
        let __robot_component_registration = {
            let __methods: ::std::vec::Vec<::framework_core::robot::Method> = ::std::vec![
                #(#method_entries),*
            ];
            ::framework_core::robot::register_component(#component_name_str, __methods)
        };
    };
    // The Effect's closure captures the registration guard by move.
    // While a `Scope` is active (the build walker runs each Primitive
    // inside one), the returned `Effect` handle is a no-op on drop —
    // the scope owns the slot and frees it (and the captured guard)
    // on scope drop. That ties the component's registration lifetime
    // to its mounted lifetime.
    let keepalive_stmt: Stmt = syn::parse_quote! {
        #[cfg(feature = "robot")]
        let _ = ::framework_core::Effect::new(move || {
            let _ = &__robot_component_registration;
        });
    };
    item_fn.block.stmts.insert(idx + 1, registration_stmt);
    item_fn.block.stmts.insert(idx + 2, keepalive_stmt);

    // Now wrap the trailing expression with Bindable::new.
    //
    // The tail may be either an expression statement (`view(...)`) or
    // a macro statement (`ui! { ... }` / `jsx! { ... }`). Macros
    // haven't been expanded yet at this point — `#[component]` sees
    // the raw tokens — so a tail-position macro shows up as
    // `Stmt::Macro`, not `Stmt::Expr`. Handle both.
    let Some(last) = item_fn.block.stmts.last_mut() else {
        return; // shouldn't happen — methods! present implies body has stmts
    };
    match last {
        Stmt::Expr(expr, None) => {
            let inner = std::mem::replace(expr, syn::parse_quote!(()));
            *expr = syn::parse_quote! {
                ::framework_core::Bindable::new(
                    ::framework_core::IntoPrimitive::into_primitive(#inner),
                    __component_handle,
                )
            };
        }
        Stmt::Macro(m) if m.semi_token.is_none() => {
            // Tail-position macro invocation — e.g. `jsx! { ... }` as
            // the implicit return. Reinterpret it as an expression so
            // we can wrap it the same way.
            let mac = m.mac.clone();
            let inner: syn::Expr = syn::parse_quote!(#mac);
            *last = Stmt::Expr(
                syn::parse_quote! {
                    ::framework_core::Bindable::new(
                        ::framework_core::IntoPrimitive::into_primitive(#inner),
                        __component_handle,
                    )
                },
                None,
            );
        }
        _ => {}
    }
}

/// `reset` → `__reset` field name. Prefixed so it can't collide with
/// any user-visible identifier in the handle.
fn field_ident(method_name: &Ident) -> Ident {
    format_ident!("__{}", method_name)
}
