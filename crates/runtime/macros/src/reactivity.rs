//! Reactivity rewriter for the `#[component]` macro.
//!
//! Walks the function body and adapts direct `text(expr)` /
//! `button(label, on_click)` calls (those written OUTSIDE `ui!` — `ui!`'s own
//! `emit_text` handles text inside the macro) so a component can take props by
//! reference (`fn counter(props: &CounterProps)`) and still use them inside
//! `'static` reactive closures.
//!
//! 0.1.0 is **type-driven**, not `.get()`-scanned: a **closure** arg is the
//! reactive form (`text(move || …)`), a value arg is static. For a closure arg
//! we clone every parameter-rooted path it reads at the closure boundary so the
//! closure can be `'static` — but we add no wrapper; the author's closure IS the
//! boundary. A `button(...)` callback always gets its param paths cloned. A bare
//! non-closure arg that reads a signal (`text(count.get())`) is static now and
//! would be a silent freeze, so it's replaced with a `compile_error!` pointing at
//! the closure form — a permanent footgun guard that turns the one silent mistake
//! into a loud error (it decides no reactivity; the closure-vs-value TYPE does).

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::visit_mut::{self, VisitMut};
use syn::{Expr, ExprCall, ItemFn};

use crate::path_analysis::{
    collect_param_paths, collect_signal_reads, substitute_in_expr, FieldPath,
};

/// Walks the function body, rewriting reactive `text(...)` and
/// `button(...)` calls. If the function's declared return type is
/// `Element`, also wraps the trailing expression with
/// `IntoElement::into_element(...)` so components can return
/// either a bare `Element` or a `Bound<H>` (from a primitive
/// constructor like `view(...)`) and have it coerced automatically —
/// matching the coercion `ui!` applies at the top level.
///
/// Components that declare a richer return type — e.g.
/// `Bindable<CounterHandle>` — are left alone so the body's actual
/// return value reaches the caller without being flattened.
pub(crate) fn rewrite(item_fn: &mut ItemFn) {
    let param_idents = extract_param_idents(item_fn);
    let mut rewriter = TextRewriter { param_idents };
    rewriter.visit_block_mut(&mut item_fn.block);
    if returns_primitive(item_fn) {
        coerce_return_to_primitive(item_fn);
    }
}

/// Checks whether the function's declared return type is bare
/// `Element`. We do a simple token-level match — this catches the
/// common spellings (`Element`, `runtime_core::Element`,
/// `::runtime_core::Element`) without trying to be a full type
/// resolver. Anything else — `Bindable<H>`, `Bound<H>`, `impl Into<…>`,
/// etc. — is left as-is.
fn returns_primitive(item_fn: &ItemFn) -> bool {
    use syn::ReturnType;
    let ty = match &item_fn.sig.output {
        ReturnType::Type(_, ty) => ty,
        ReturnType::Default => return false,
    };
    let rendered = quote::quote!(#ty).to_string();
    // Strip whitespace to normalize `:: runtime_core :: Element`
    // vs `::runtime_core::Element`.
    let normalized: String = rendered.chars().filter(|c| !c.is_whitespace()).collect();
    matches!(
        normalized.as_str(),
        "Element" | "runtime_core::Element" | "::runtime_core::Element"
    )
}

/// Wraps the function's final expression (the implicit return) with
/// `IntoElement::into_element(...)`. We only wrap if the body's
/// trailing expression is a "real" expression (i.e. the function
/// implicitly returns it); we don't try to find explicit `return`
/// statements deeper in the body. That's fine — if you write
/// `return view(...)` in the middle of a component, you can still add
/// `.into()` yourself. The common case is a tail expression.
fn coerce_return_to_primitive(item_fn: &mut ItemFn) {
    use syn::Stmt;
    let block = &mut item_fn.block;
    let Some(last) = block.stmts.last_mut() else { return };
    if let Stmt::Expr(expr, None) = last {
        let inner = std::mem::replace(expr, syn::parse_quote!(()));
        *expr = syn::parse_quote! {
            ::runtime_core::IntoElement::into_element(#inner)
        };
    }
}

/// Pulls the plain-ident parameter names out of a function signature.
/// `fn counter(props: &CounterProps)` → `[props]`. Skips destructuring
/// patterns (e.g. `fn(SomeStruct { a, b }: T)`).
fn extract_param_idents(item_fn: &ItemFn) -> Vec<String> {
    item_fn
        .sig
        .inputs
        .iter()
        .filter_map(|arg| match arg {
            syn::FnArg::Typed(pat_type) => match &*pat_type.pat {
                syn::Pat::Ident(pi) => Some(pi.ident.to_string()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

struct TextRewriter {
    param_idents: Vec<String>,
}

impl VisitMut for TextRewriter {
    fn visit_expr_mut(&mut self, expr: &mut Expr) {
        // `visit_mut` doesn't descend into macro bodies (they're opaque
        // tokens). For `vec!` and `children!` — the canonical list-shaped
        // macros — we re-parse the body as a comma-separated list of
        // expressions, rewrite each, and re-emit. Other macros are opaque.
        if let Expr::Macro(em) = expr {
            if em.mac.path.is_ident("vec") || em.mac.path.is_ident("children") {
                if let Ok(mut parsed) = syn::parse2::<CommaExprs>(em.mac.tokens.clone()) {
                    for e in &mut parsed.exprs {
                        self.visit_expr_mut(e);
                    }
                    let exprs = parsed.exprs;
                    em.mac.tokens = quote! { #(#exprs),* };
                }
            }
        }

        // Recurse first so inner text() calls are rewritten before the
        // outer call sees them.
        visit_mut::visit_expr_mut(self, expr);

        // A rewrite may replace the WHOLE expr (the migration `compile_error!`),
        // so compute the replacement without holding the `&mut ExprCall` borrow
        // across the reassignment.
        let replacement: Option<Expr> = if let Expr::Call(call) = expr {
            if is_button_call(call) && call.args.len() == 2 {
                rewrite_button_args(call, &self.param_idents)
            } else if is_text_call(call) && call.args.len() == 1 {
                rewrite_text_arg(call, &self.param_idents)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(r) = replacement {
            *expr = r;
        }
    }
}

/// Rewrite a direct `text(expr)` call (outside `ui!`; `ui!`'s own `emit_text`
/// handles text inside the macro). 0.1.0 is **type-driven**: a closure arg is
/// reactive, a value arg is static.
///
/// - **Closure arg** (`text(move || …)`): the reactive form. Clone every
///   parameter-rooted path it reads at the closure boundary so the closure is
///   `'static` (a component takes props by reference; `props.value` must be
///   cloned in, not borrowed) — but do NOT add a wrapper closure; the author's
///   closure IS the boundary. `IntoTextSource for Fn() -> String` routes it.
/// - **Macro arg** (`text(rx!(…))`, `text(text_fmt!(…))`): reactive by TYPE —
///   left untouched.
/// - **Bare value that reads a signal** (`text(count.get())`): auto-wrapped in
///   0.0.1, silently static now — replaced with a `compile_error!` pointing at
///   the closure form (migration guard).
/// - **Anything else**: static, untouched.
///
/// Returns `Some(replacement)` to swap the whole expr (the guard), else `None`.
fn rewrite_text_arg(call: &mut ExprCall, param_idents: &[String]) -> Option<Expr> {
    let arg = call.args.first().expect("text() takes one arg");
    match arg {
        Expr::Closure(_) => {
            let paths = collect_param_paths(arg, param_idents);
            if paths.is_empty() {
                return None; // closure over locals / Copy signals — already 'static
            }
            let bindings = emit_clone_bindings(&paths);
            let mut rewritten = arg.clone();
            substitute_in_expr(&mut rewritten, &paths);
            let func = call.func.clone();
            let new_expr: Expr = syn::parse2(quote! {
                #func({ #(#bindings)* #rewritten })
            })
            .expect("text closure param-clone produced invalid expr");
            *call = match new_expr {
                Expr::Call(c) => c,
                _ => unreachable!("we just emitted a Call"),
            };
            None
        }
        Expr::Macro(_) => None,
        _ if !collect_signal_reads(arg).is_empty() => Some(syn::parse_quote! {
            ::std::compile_error!(
                "0.1.0: reactive text must be a closure — write `text(move || …)` \
                 instead of `text(…sig.get()…)`. A bare `.get()` in text is now static; \
                 `rx!(…)` / `text_fmt!(…)` values stay reactive by type."
            )
        }),
        _ => None,
    }
}

/// Rewrite a direct `button(label, on_click)` call. Same type-driven rule for the
/// label as [`rewrite_text_arg`] (closure = reactive, macro = reactive-by-type,
/// bare signal read = migration `compile_error!`), plus the callback always gets
/// its parameter-rooted paths cloned so it doesn't borrow.
///
/// Returns `Some(replacement)` to swap the whole expr (the label guard), else
/// `None` (rewrote in place / left static).
fn rewrite_button_args(call: &mut ExprCall, param_idents: &[String]) -> Option<Expr> {
    let label_orig = call.args[0].clone();
    let callback = call.args[1].clone();

    // --- Label.
    let label_expr: Expr = match &label_orig {
        Expr::Closure(closure) => {
            // Reactive label. Coerce the closure body to `String` (a
            // `move || sig.get()` may yield any `Display` type) and clone any
            // param-rooted paths so it's `'static`.
            let body = &closure.body;
            let paths = collect_param_paths(&label_orig, param_idents);
            let bindings = emit_clone_bindings(&paths);
            let mut coerced: Expr = syn::parse_quote! {
                move || ::std::string::ToString::to_string(&{ #body })
            };
            substitute_in_expr(&mut coerced, &paths);
            syn::parse2(quote! {{ #(#bindings)* #coerced }})
                .expect("button label closure rewrite produced invalid expr")
        }
        Expr::Macro(_) => label_orig,
        _ if !collect_signal_reads(&label_orig).is_empty() => {
            return Some(syn::parse_quote! {
                ::std::compile_error!(
                    "0.1.0: a reactive button label must be a closure — write \
                     `button(move || …, on_click)` instead of `button(…sig.get()…, on_click)`."
                )
            });
        }
        _ => label_orig,
    };

    // --- Callback: clone parameter-rooted paths it reads.
    let callback_paths = collect_param_paths(&callback, param_idents);
    let callback_expr: Expr = if callback_paths.is_empty() {
        callback
    } else {
        let bindings = emit_clone_bindings(&callback_paths);
        let mut rewritten = callback;
        substitute_in_expr(&mut rewritten, &callback_paths);
        syn::parse2(quote! {{
            #(#bindings)*
            #rewritten
        }})
        .expect("button callback rewrite produced invalid expr")
    };

    let func = call.func.clone();
    let new_expr: Expr = syn::parse2(quote! {
        #func(#label_expr, #callback_expr)
    })
    .expect("button rewrite produced invalid expr");
    *call = match new_expr {
        Expr::Call(c) => c,
        _ => unreachable!("we just emitted a Call"),
    };
    None
}

/// Produces `let __rc_X = X.clone();` lines for each path.
fn emit_clone_bindings(paths: &[FieldPath]) -> Vec<TokenStream2> {
    paths
        .iter()
        .map(|p| {
            let local = p.local_ident();
            let path = p.as_tokens();
            quote! { let #local = #path.clone(); }
        })
        .collect()
}

fn is_text_call(call: &ExprCall) -> bool {
    is_named_call(call, "text")
}

fn is_button_call(call: &ExprCall) -> bool {
    is_named_call(call, "button")
}

fn is_named_call(call: &ExprCall, name: &str) -> bool {
    if let Expr::Path(p) = &*call.func {
        if p.qself.is_none() && p.path.segments.len() == 1 {
            return p.path.segments[0].ident == name;
        }
    }
    false
}

/// Parser for `expr, expr, expr` — the body of a `vec![...]` or
/// `children![...]` literal.
struct CommaExprs {
    exprs: Vec<Expr>,
}
impl syn::parse::Parse for CommaExprs {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let p: syn::punctuated::Punctuated<Expr, syn::Token![,]> =
            syn::punctuated::Punctuated::parse_terminated(input)?;
        Ok(CommaExprs { exprs: p.into_iter().collect() })
    }
}
