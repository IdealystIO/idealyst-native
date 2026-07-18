//! `#[method]` fn lifting inside `#[component]` — the component's
//! imperative surface (commands).
//!
//! A component author declares methods as nested fns marked `#[method]`:
//!
//! ```ignore
//! #[component]
//! pub fn Counter() -> Element {
//!     let value = signal(0);
//!
//!     #[method]
//!     /// Reset the count to zero.
//!     fn reset() { value.set(0); }
//!
//!     #[method]
//!     fn bump_by(n: i32) { value.update(|v| *v += n); }
//!
//!     ui! { /* ... */ }
//! }
//!
//! // Caller — the ordinary tag form binds via the auto-injected prop:
//! let h: Ref<CounterHandle> = Ref::new();
//! ui! { Counter(bind_to = h) }
//! h.get().map(|c| c.bump_by(3));   // .get(), not .with() — methods write signals
//! ```
//!
//! No `pub` (the generated handle carries the public surface), no
//! `&self` (captures come from the closure over the component body —
//! which a real nested fn could never do; the attribute is what licenses
//! the transform), and `()` returns only: **methods are commands, not
//! queries** — reads stay signals, so the handle can never become a
//! subscription-bypassing read side-channel.
//!
//! This module:
//!  - finds the `#[method]` fn items in the function body,
//!  - generates a sibling `CounterHandle` struct with `Rc<dyn Fn(...)>`
//!    fields, a `Clone` impl, and `pub fn name(...)` accessors,
//!  - removes the fn items and constructs the handle just before the
//!    tail (maximal capture scope), filling the auto-injected
//!    `bind_to: Option<Ref<CounterHandle>>` prop when present,
//!  - registers the methods with the robot bridge (JSON-invocable),
//!  - wraps the tail in `__component_root` (element↔component link);
//!    only the LEGACY explicit-props form still wraps in
//!    `Bindable::new` (fn-call `.bind()` binding — no prop injection
//!    is possible into an author-written props struct).
//!
//! The handle's name is derived from the component's fn name by
//! converting `snake_case` to `PascalCase` and appending `Handle`
//! (`Counter` → `CounterHandle`).

use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};

use syn::{Block, FnArg, Ident, ItemFn, Pat, Stmt, Type};

/// Extract the body's `#[method]` fns. If present:
///  - synthesizes the handle struct + impls (returned in the
///    `TokenStream2`),
///  - removes the fn items, constructing the handle just before the
///    tail (plus the `bind_to` fill on the new path),
///  - wraps the tail (`__component_root`; legacy adds `Bindable::new`).
///
/// If no `#[method]` fns are present, returns an empty `TokenStream2`
/// and leaves the function untouched.
pub(crate) fn extract_and_rewrite(
    item_fn: &mut ItemFn,
    bind_to_injected: bool,
) -> syn::Result<(TokenStream2, Vec<MethodInfo>)> {
    let Some((indices, methods)) = find_and_parse_methods(item_fn)? else {
        return Ok((TokenStream2::new(), Vec::new()));
    };

    let handle_name = derive_handle_name(&item_fn.sig.ident);
    let fn_name = item_fn.sig.ident.clone();
    let extra = generate_handle_type(&handle_name, &item_fn.sig.ident, &methods);

    let infos: Vec<MethodInfo> = methods
        .iter()
        .map(|m| MethodInfo {
            name: m.name.to_string(),
            docs: method_docs(&m.attrs),
            params: m
                .args
                .iter()
                .map(|(ident, ty)| {
                    let ty_str = quote::quote! { #ty }.to_string();
                    (ident.to_string(), ty_str)
                })
                .collect(),
        })
        .collect();

    rewrite_body(item_fn, &indices, &handle_name, &fn_name, &methods, bind_to_injected);

    Ok((extra, infos))
}

/// Cheap pre-scan used by the `#[component]` pipeline BEFORE the props
/// machinery runs: does the body declare any `#[method]` fns? Drives the
/// `bind_to` prop injection (which must happen before inline-props
/// expansion turns parameters into fields).
pub(crate) fn has_method_fns(item_fn: &ItemFn) -> bool {
    item_fn.block.stmts.iter().any(|s| as_method_fn(s).is_some())
}

/// `Some(&ItemFn)` when the statement is a nested fn carrying `#[method]`.
fn as_method_fn(stmt: &Stmt) -> Option<&ItemFn> {
    let Stmt::Item(syn::Item::Fn(f)) = stmt else { return None };
    f.attrs.iter().any(|a| a.path().is_ident("method")).then_some(f)
}

/// Walks the function body for `#[method]`-marked nested fns. Returns
/// (statement indices, parsed methods) if any are found. Also rejects a
/// leftover `methods! { … }` block with a pointer at the new spelling —
/// without this, the author would get rustc's unhelpful "cannot find
/// macro `methods`".
fn find_and_parse_methods(item_fn: &ItemFn) -> syn::Result<Option<(Vec<usize>, Vec<MethodDef>)>> {
    for stmt in &item_fn.block.stmts {
        if let Stmt::Macro(m) = stmt {
            if m.mac.path.is_ident("methods") {
                return Err(syn::Error::new_spanned(
                    &m.mac.path,
                    "`methods! { … }` was replaced by `#[method]` on nested fns: \
                     write `#[method] fn name(args…) { … }` directly in the component \
                     body (no `&self`, no visibility — the generated handle carries \
                     the public surface)",
                ));
            }
        }
    }

    let mut indices = Vec::new();
    let mut methods = Vec::new();
    for (i, stmt) in item_fn.block.stmts.iter().enumerate() {
        let Some(f) = as_method_fn(stmt) else { continue };
        indices.push(i);
        methods.push(parse_method_fn(f)?);
    }
    if indices.is_empty() {
        return Ok(None);
    }
    Ok(Some((indices, methods)))
}

/// Validate + digest one `#[method] fn`. The nested-fn spelling reads as
/// plain Rust, so the validations teach what the transformation permits:
/// the body becomes a `move` closure over the component's state (which a
/// real nested fn could never capture — the attribute is what licenses
/// it).
fn parse_method_fn(f: &ItemFn) -> syn::Result<MethodDef> {
    if !matches!(f.vis, syn::Visibility::Inherited) {
        return Err(syn::Error::new_spanned(
            &f.vis,
            "`#[method]` fns take no visibility — the generated handle struct \
             carries the public surface",
        ));
    }
    if let Some(a) = &f.sig.asyncness {
        return Err(syn::Error::new_spanned(a, "`#[method]` fns cannot be async (v1)"));
    }
    if let Some(u) = &f.sig.unsafety {
        return Err(syn::Error::new_spanned(u, "`#[method]` fns cannot be unsafe"));
    }
    if !f.sig.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &f.sig.generics,
            "`#[method]` fns cannot be generic — the handle stores them type-erased",
        ));
    }
    if let syn::ReturnType::Type(arrow, _) = &f.sig.output {
        return Err(syn::Error::new_spanned(
            arrow,
            "`#[method]` fns return `()` only — methods are COMMANDS; expose reads \
             as signals (a `ReadSignal` prop or a memo), not through the handle",
        ));
    }

    let mut args = Vec::new();
    for a in &f.sig.inputs {
        match a {
            FnArg::Receiver(r) => {
                return Err(syn::Error::new_spanned(
                    r,
                    "`#[method]` fns take no `&self` — state comes from the closure \
                     capturing the component body, not from a receiver",
                ));
            }
            FnArg::Typed(pt) => {
                let ident = match &*pt.pat {
                    Pat::Ident(pi) => pi.ident.clone(),
                    _ => {
                        return Err(syn::Error::new_spanned(
                            &pt.pat,
                            "`#[method]` arguments must use plain identifier patterns",
                        ));
                    }
                };
                args.push((ident, (*pt.ty).clone()));
            }
        }
    }

    // Docs ride along for the MCP catalog; the `#[method]` marker itself
    // is consumed here (rustc never sees it).
    let attrs = f
        .attrs
        .iter()
        .filter(|a| !a.path().is_ident("method"))
        .cloned()
        .collect();

    Ok(MethodDef {
        attrs,
        name: f.sig.ident.clone(),
        args,
        body: (*f.block).clone(),
    })
}

/// One digested `#[method]` fn declaration.
struct MethodDef {
    /// Outer attributes (most importantly, `///` doc comments) attached
    /// to the `fn name(...)` line. Captured so the MCP catalog can
    /// surface per-method documentation.
    attrs: Vec<syn::Attribute>,
    name: Ident,
    /// Args other than `&self`. Each is (binding, type).
    args: Vec<(Ident, Type)>,
    body: Block,
}

/// Per-method summary exported to the parent macro for MCP registration
/// — read-only view over a parsed `MethodDef` that callers outside this
/// module can consume without touching the inner `Block`.
pub(crate) struct MethodInfo {
    pub name: String,
    pub docs: String,
    /// `(name, pretty-printed type)` for each non-`&self` arg.
    pub params: Vec<(String, String)>,
}

fn method_docs(attrs: &[syn::Attribute]) -> String {
    let mut lines: Vec<String> = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(nv) = &attr.meta {
            if let syn::Expr::Lit(syn::ExprLit { lit: syn::Lit::Str(s), .. }) = &nv.value {
                let raw = s.value();
                lines.push(raw.strip_prefix(' ').unwrap_or(&raw).to_string());
            }
        }
    }
    lines.join("\n")
}

/// `counter` → `CounterHandle`. snake_case to PascalCase + Handle.
pub(crate) fn derive_handle_name(fn_name: &Ident) -> Ident {
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
///
/// The `///` docs from each `#[method]` fn ride onto the generated
/// accessor, and the struct gets a synthesized doc naming the component
/// and the invocation rule — so completion and hover in the IDE show
/// real documentation, not bare signatures.
fn generate_handle_type(
    name: &Ident,
    component: &Ident,
    methods: &[MethodDef],
) -> TokenStream2 {
    let struct_doc = format!(
        "Imperative handle for [`{component}`]'s `#[method]` fns. Obtain it \
         through the auto-injected `bind_to` prop \
         (`ui! {{ {component}(bind_to = my_ref) }}`); invoke via \
         `my_ref.get()` — not `.with()`, since methods write signals."
    );
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
        let docs = m.attrs.iter().filter(|a| a.path().is_ident("doc"));
        let arg_names = m.args.iter().map(|(n, _)| n);
        let arg_names2 = m.args.iter().map(|(n, _)| n);
        let arg_tys = m.args.iter().map(|(_, ty)| ty);
        quote! {
            #(#docs)*
            pub fn #method_name(&self, #(#arg_names: #arg_tys),*) {
                (self.#f)(#(#arg_names2),*);
            }
        }
    });
    let part_params = methods.iter().map(|m| {
        let f = field_ident(&m.name);
        let arg_tys = m.args.iter().map(|(_, ty)| ty);
        quote! { #f: ::std::rc::Rc<dyn Fn(#(#arg_tys),*)> }
    });
    let part_names = methods.iter().map(|m| field_ident(&m.name));

    // The struct lives in a hidden CHILD module so its closure-plumbing
    // fields are truly private: the component sits in the PARENT module,
    // and same-module code sees private fields — which polluted
    // completion with `__bump`-style internals for any caller in the
    // component's own file. The re-export keeps the author-facing path
    // (`Ref<CounterHandle>`) unchanged; construction goes through the
    // doc-hidden `__from_parts` (args in method-declaration order).
    let mod_name = handle_mod_ident(component);
    quote! {
        #[doc(hidden)]
        pub mod #mod_name {
            #[doc = #struct_doc]
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

                /// Internal constructor for the `#[component]` codegen —
                /// not part of the handle's public surface.
                #[doc(hidden)]
                pub fn __from_parts(#(#part_params),*) -> Self {
                    Self { #(#part_names,)* }
                }
            }
        }

        pub use #mod_name::#name;
    }
}

/// `Counter` → `__counter_handle` — the hidden module hosting the
/// generated handle so its fields stay private to the codegen.
fn handle_mod_ident(component: &Ident) -> Ident {
    let raw = component.to_string();
    let mut snake = String::with_capacity(raw.len() + 10);
    for (i, c) in raw.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i != 0 {
                snake.push('_');
            }
            snake.extend(c.to_lowercase());
        } else {
            snake.push(c);
        }
    }
    format_ident!("__{}_handle", snake, span = component.span())
}

/// Mutates the function body in place:
///   1. Removes the `#[method]` fn items (their bodies move into `move`
///      closures, so captures come from the component body — which a
///      real nested fn could never do; that's the licensed transform).
///   2. Just before the tail expression (maximal capture scope — every
///      binding in the body is visible there, regardless of where the
///      `#[method]` fns were declared), inserts: the handle
///      construction, the `bind_to` fill (new path), and the robot
///      registration + keepalive.
///   3. Wraps the tail: new path (`bind_to` injected) keeps the plain
///      `Element` return, wrapping only in `__component_root` for the
///      robot element↔component link; the legacy explicit-props path
///      still wraps in `Bindable::new` (fn-call `.bind()` binding).
fn rewrite_body(
    item_fn: &mut ItemFn,
    indices: &[usize],
    handle_name: &Ident,
    fn_name: &Ident,
    methods: &[MethodDef],
    bind_to_injected: bool,
) {
    // Build the handle construction block.
    let field_inits = methods.iter().map(|m| {
        let body = &m.body;
        let arg_names = m.args.iter().map(|(n, _)| n);
        let arg_tys = m.args.iter().map(|(_, ty)| ty);
        let arg_tys2 = m.args.iter().map(|(_, ty)| ty);
        // The cast is necessary so the closure coerces to the trait
        // object the constructor expects — closure types are unique per
        // closure, but `Rc<dyn Fn(...)>` is a single concrete parameter
        // type.
        quote! {
            {
                let __c = ::std::rc::Rc::new(move |#(#arg_names: #arg_tys),*| #body);
                __c as ::std::rc::Rc<dyn Fn(#(#arg_tys2),*)>
            }
        }
    });

    // Construction goes through the doc-hidden `__from_parts` (the
    // fields are private to the generated child module), passing the
    // method closures in declaration order.
    let handle_construction: Stmt = syn::parse_quote! {
        let __component_handle = #handle_name::__from_parts(#(#field_inits),*);
    };

    // Remove the `#[method]` fn items (descending so indices stay valid).
    for &i in indices.iter().rev() {
        item_fn.block.stmts.remove(i);
    }

    // Insertion point: just before the tail expression, so the handle's
    // closures (and the fill below) see every binding in the body.
    let idx = item_fn.block.stmts.len().saturating_sub(1);
    item_fn.block.stmts.insert(idx, handle_construction);

    // New path: fill the injected `bind_to` prop with the handle, so the
    // ordinary `ui! { Tag(bind_to = my_ref) }` tag form binds — no
    // `Bindable` return, no fn-call form. `Ref` fills exactly once, at
    // build (same contract as idea-ui's unconditional primitive binds).
    if bind_to_injected {
        let fill_stmt: Stmt = syn::parse_quote! {
            if let ::core::option::Option::Some(__r) = bind_to {
                __r.fill(::std::clone::Clone::clone(&__component_handle));
            }
        };
        item_fn.block.stmts.insert(idx + 1, fill_stmt);
    }
    let idx = if bind_to_injected { idx + 1 } else { idx };

    // Insert robot auto-registration directly after the handle binding.
    // Each `#[method]` fn becomes a JSON-callable adapter that
    // deserializes each argument by name, then forwards to the handle's
    // method. The whole block is `#[cfg]`-gated to the consuming crate's
    // `robot` feature so non-robot builds pay nothing.
    let component_name_str = fn_name.to_string();
    let method_entries = methods.iter().map(|m| {
        let method_name_str = m.name.to_string();
        let arg_names_str: Vec<String> = m.args.iter().map(|(n, _)| n.to_string()).collect();
        // Render each arg's Rust type as a source-form string. `quote!`
        // separates every token with a single space; we only want to drop
        // the spaces that hug punctuation, so `Vec < String >` collapses
        // to `Vec<String>` and `& 'a str` to `&'a str`, while a real gap
        // between two word tokens (`dyn Fn`) is preserved. Crucially we
        // must NOT split a single ident: the old code stripped every space
        // then re-inserted one between adjacent alphanumerics, turning the
        // one-token type `i32` into `i 3 2`. Token-level formatting isn't
        // worth a `prettyplease` dep for one-line type renderings.
        let arg_types_str: Vec<String> = m.args.iter().map(|(_, ty)| {
            const PUNCT: &[char] = &['<', '>', ',', '(', ')', '&', '\'', ':', ';'];
            let raw = quote!(#ty).to_string();
            let chars: Vec<char> = raw.chars().collect();
            let mut out = String::with_capacity(raw.len());
            for (i, &ch) in chars.iter().enumerate() {
                if ch == ' ' {
                    // Drop this space iff the char on either side is
                    // punctuation; keep it when it sits between two word
                    // tokens (it's a meaningful separator there).
                    let prev = out.chars().last();
                    let next = chars.get(i + 1).copied();
                    let hugs_punct = prev.map_or(true, |p| PUNCT.contains(&p))
                        || next.map_or(true, |n| PUNCT.contains(&n));
                    if hugs_punct {
                        continue;
                    }
                }
                out.push(ch);
            }
            out
        }).collect();
        let arg_idents: Vec<&Ident> = m.args.iter().map(|(n, _)| n).collect();
        let arg_idents_for_call = arg_idents.clone();
        let arg_tys: Vec<&Type> = m.args.iter().map(|(_, ty)| ty).collect();
        let method_ident = &m.name;
        let arg_extractions = arg_idents.iter().zip(arg_tys.iter()).zip(arg_names_str.iter()).map(|((ident, ty), name)| {
            quote! {
                let #ident: #ty = ::runtime_core::__serde_json::from_value(
                    __args.get(#name).cloned().unwrap_or(::runtime_core::__serde_json::Value::Null),
                ).map_err(|e| ::std::format!("arg '{}': {}", #name, e))?;
            }
        });
        let arg_pairs = arg_names_str.iter().zip(arg_types_str.iter()).map(|(n, t)| {
            quote! { (#n, #t) }
        });
        quote! {
            ::runtime_core::robot::Method {
                name: #method_name_str,
                args: &[#(#arg_pairs),*],
                invoke: {
                    let __h = ::std::clone::Clone::clone(&__component_handle);
                    ::std::rc::Rc::new(move |__args: &::runtime_core::__serde_json::Value| -> ::std::result::Result<(), ::std::string::String> {
                        #(#arg_extractions)*
                        __h.#method_ident(#(#arg_idents_for_call),*);
                        ::std::result::Result::Ok(())
                    })
                },
            }
        }
    });

    // Emitted UNCONDITIONALLY — the registration surface
    // (`register_component` / `Method` / `ComponentRegistration`) always
    // exists in `runtime-core` (a no-op stub when the `robot` feature is
    // off). Gating this on the *consuming* crate's `robot` feature meant
    // a scaffolded app / idea-ui (which never declare that feature)
    // silently never registered their component methods — `invoke_method`
    // saw nothing. Now methods register whenever `runtime-core/robot` is
    // on, regardless of the defining crate's features.
    let registration_stmt: Stmt = syn::parse_quote! {
        let __robot_component_registration = {
            let __methods: ::std::vec::Vec<::runtime_core::robot::Method> = ::std::vec![
                #(#method_entries),*
            ];
            ::runtime_core::robot::register_component(#component_name_str, __methods)
        };
    };
    // The Effect's closure captures the registration guard by move.
    // While a `Scope` is active (the build walker runs each Element
    // inside one), the returned `Effect` handle is a no-op on drop —
    // the scope owns the slot and frees it (and the captured guard)
    // on scope drop. That ties the component's registration lifetime
    // to its mounted lifetime.
    // Capture the instance id BEFORE the keepalive moves the registration
    // into its Effect closure. Used by `__component_root` to tag the
    // component's root element so the robot walker links element↔component
    // (for the inspector's "select an element → call its methods"). `Copy`,
    // so capturing it doesn't disturb the move into the keepalive.
    let instance_id_stmt: Stmt = syn::parse_quote! {
        let __robot_component_instance = __robot_component_registration.id();
    };
    // Keepalive effect: its closure captures the registration guard by move.
    // While a `Scope` is active (the build walker runs each Element inside
    // one), the effect's slot is owned by that scope, which frees it (and the
    // captured guard) on scope drop — tying the component's registration
    // lifetime to its mounted lifetime. `__component_keepalive_effect` is the
    // framework's internal codegen entry (adopt-or-own, no scope assertion);
    // author code can't reach the raw `Effect` constructor.
    let keepalive_stmt: Stmt = syn::parse_quote! {
        ::runtime_core::__component_keepalive_effect(move || {
            let _ = &__robot_component_registration;
        });
    };
    item_fn.block.stmts.insert(idx + 1, registration_stmt);
    item_fn.block.stmts.insert(idx + 2, instance_id_stmt);
    item_fn.block.stmts.insert(idx + 3, keepalive_stmt);

    // Now wrap the trailing expression with Bindable::new.
    //
    // The tail may be either an expression statement (`view(...)`) or
    // a macro statement (`ui! { ... }` / `jsx! { ... }`). Macros
    // haven't been expanded yet at this point — `#[component]` sees
    // the raw tokens — so a tail-position macro shows up as
    // `Stmt::Macro`, not `Stmt::Expr`. Handle both.
    let Some(last) = item_fn.block.stmts.last_mut() else {
        return; // shouldn't happen — #[method] present implies body has stmts
    };
    let wrap = |inner: syn::Expr| -> syn::Expr {
        if bind_to_injected {
            // New path: the handle already reached the caller through the
            // injected `bind_to` prop, so the component returns a plain
            // `Element` — only the robot element↔component link wraps.
            syn::parse_quote! {
                ::runtime_core::__component_root(
                    ::runtime_core::IntoElement::into_element(#inner),
                    __robot_component_instance,
                )
            }
        } else {
            // Legacy explicit-props path: the handle escapes through the
            // `Bindable` return (fn-call `.bind()` binding).
            syn::parse_quote! {
                ::runtime_core::Bindable::new(
                    ::runtime_core::__component_root(
                        ::runtime_core::IntoElement::into_element(#inner),
                        __robot_component_instance,
                    ),
                    __component_handle,
                )
            }
        }
    };
    match last {
        Stmt::Expr(expr, None) => {
            let inner = std::mem::replace(expr, syn::parse_quote!(()));
            *expr = wrap(inner);
        }
        Stmt::Macro(m) if m.semi_token.is_none() => {
            // Tail-position macro invocation — e.g. `ui! { ... }` as
            // the implicit return. Reinterpret it as an expression so
            // we can wrap it the same way.
            let mac = m.mac.clone();
            let inner: syn::Expr = syn::parse_quote!(#mac);
            *last = Stmt::Expr(wrap(inner), None);
        }
        _ => {}
    }
}

/// `reset` → `__reset` field name. Prefixed so it can't collide with
/// any user-visible identifier in the handle.
fn field_ident(method_name: &Ident) -> Ident {
    format_ident!("__{}", method_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn run(
        tokens: TokenStream2,
        bind_to_injected: bool,
    ) -> syn::Result<(String, String)> {
        let mut item_fn: ItemFn = syn::parse2(tokens).unwrap();
        let (extra, _infos) = extract_and_rewrite(&mut item_fn, bind_to_injected)?;
        let squash = |t: TokenStream2| -> String {
            t.to_string().chars().filter(|c| !c.is_whitespace()).collect()
        };
        Ok((squash(extra), squash(quote!(#item_fn))))
    }

    #[test]
    fn generates_handle_struct_accessors_and_forwards_docs() {
        let (extra, body) = run(
            quote! {
                fn Counter() -> Element {
                    let value = signal(0);
                    /// Reset the count to zero.
                    #[method]
                    fn reset() { value.set(0); }
                    #[method]
                    fn bump(n: i32) { value.update(|v| *v += n); }
                    view(Vec::new())
                }
            },
            true,
        )
        .unwrap();
        assert!(extra.contains("pubmod__counter_handle"), "handle lives in a hidden module: {extra}");
        assert!(extra.contains("pubstructCounterHandle"), "{extra}");
        assert!(extra.contains("pubuse__counter_handle::CounterHandle"), "re-exported at the original path: {extra}");
        assert!(extra.contains("pubfnreset(&self,)"), "{extra}");
        assert!(extra.contains("pubfnbump(&self,n:i32)"), "{extra}");
        // Docs ride onto the accessor AND the struct gets a synthesized doc.
        assert!(extra.contains("Resetthecounttozero."), "method doc forwarded: {extra}");
        assert!(extra.contains("Imperativehandlefor[`Counter`]"), "{extra}");
        // Body: handle constructed + method items removed.
        assert!(body.contains("let__component_handle=CounterHandle::__from_parts"), "{body}");
        assert!(!body.contains("#[method]"), "method items must be consumed: {body}");
    }

    #[test]
    fn injected_path_fills_bind_to_and_skips_bindable() {
        let (_extra, body) = run(
            quote! {
                fn Tally() -> Element {
                    let value = signal(0);
                    #[method]
                    fn reset() { value.set(0); }
                    view(Vec::new())
                }
            },
            true,
        )
        .unwrap();
        assert!(body.contains("__r.fill"), "bind_to fill emitted: {body}");
        assert!(!body.contains("Bindable::new"), "new path returns plain Element: {body}");
        assert!(body.contains("__component_root"), "robot link wrap kept: {body}");
    }

    #[test]
    fn legacy_path_keeps_bindable_wrap() {
        let (_extra, body) = run(
            quote! {
                fn counter(props: &CounterProps) -> Bindable<CounterHandle> {
                    let value = signal(0);
                    #[method]
                    fn reset() { value.set(0); }
                    view(Vec::new())
                }
            },
            false,
        )
        .unwrap();
        assert!(body.contains("Bindable::new"), "legacy keeps the Bindable return: {body}");
        assert!(!body.contains("__r.fill"), "no bind_to in scope on the legacy path: {body}");
    }

    #[test]
    fn validations_reject_bad_shapes() {
        for (tokens, needle) in [
            (quote! { fn C() -> Element { #[method] pub fn x() {} view(Vec::new()) } }, "no visibility"),
            (quote! { fn C() -> Element { #[method] fn x(&self) {} view(Vec::new()) } }, "no `&self`"),
            (quote! { fn C() -> Element { #[method] fn x() -> i32 { 1 } view(Vec::new()) } }, "COMMANDS"),
            (quote! { fn C() -> Element { #[method] async fn x() {} view(Vec::new()) } }, "async"),
        ] {
            let err = run(tokens, true).unwrap_err().to_string();
            assert!(err.contains(needle), "expected {needle:?} in: {err}");
        }
    }

    #[test]
    fn leftover_methods_bang_gets_migration_error() {
        let err = run(
            quote! {
                fn C() -> Element {
                    methods! { fn reset(&self) {} }
                    view(Vec::new())
                }
            },
            false,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("replaced by `#[method]`"), "{err}");
    }
}
