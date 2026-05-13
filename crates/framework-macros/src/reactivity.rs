//! Reactivity rewriter for the `#[component]` macro.
//!
//! Walks the function body and rewrites `text(expr)` and
//! `button(label, on_click)` calls so that any signals or parameter-rooted
//! paths they read get cloned into freshly-named locals at the closure
//! boundary. This is what lets components take props by reference
//! (`fn counter(props: &CounterProps)`) and still use them inside
//! `'static` reactive closures.
//!
//! Heuristic: a `text(...)` argument is "reactive" iff it contains a
//! `.get()` method call somewhere (the convention for signal reads).
//! A `button(...)` callback is always rewritten (it's already a closure).

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::visit_mut::{self, VisitMut};
use syn::{Expr, ExprCall, ItemFn};

use crate::path_analysis::{
    collect_param_paths, collect_signal_reads, substitute_in_expr, FieldPath,
};

/// Walks the function body, rewriting reactive `text(...)` and
/// `button(...)` calls.
pub(crate) fn rewrite(item_fn: &mut ItemFn) {
    let param_idents = extract_param_idents(item_fn);
    let mut rewriter = TextRewriter { param_idents };
    rewriter.visit_block_mut(&mut item_fn.block);
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

        if let Expr::Call(call) = expr {
            if is_button_call(call) && call.args.len() == 2 {
                rewrite_button_callback(call, &self.param_idents);
                return;
            }
            if is_text_call(call) && call.args.len() == 1 {
                rewrite_text_arg(call, &self.param_idents);
            }
        }
    }
}

/// If `text(expr)`'s argument reads a signal (contains `.get()`), wrap
/// it in a reactive closure with all parameter-rooted paths cloned at the
/// closure boundary.
fn rewrite_text_arg(call: &mut ExprCall, param_idents: &[String]) {
    let arg = call.args.first().expect("text() takes one arg");
    let signal_paths = collect_signal_reads(arg);
    if signal_paths.is_empty() {
        return;
    }
    // Once we know we're building a reactive closure, also clone every
    // path rooted in a function parameter — otherwise non-reactive fields
    // like `props.label` would be captured by reference and the closure
    // couldn't be `'static`.
    let mut paths = signal_paths;
    for extra in collect_param_paths(arg, param_idents) {
        if !paths.contains(&extra) {
            paths.push(extra);
        }
    }

    let bindings = emit_clone_bindings(&paths);
    let mut rewritten_arg = arg.clone();
    substitute_in_expr(&mut rewritten_arg, &paths);
    let func = call.func.clone();

    let new_expr: Expr = syn::parse2(quote! {
        #func({
            #(#bindings)*
            move || #rewritten_arg
        })
    })
    .expect("text rewrite produced invalid expr");
    *call = match new_expr {
        Expr::Call(c) => c,
        _ => unreachable!("we just emitted a Call"),
    };
}

/// Rewrites `button(label, on_click)` so that any parameter-rooted path
/// read by `on_click` is cloned into a fresh local before the closure.
fn rewrite_button_callback(call: &mut ExprCall, param_idents: &[String]) {
    let label = call.args[0].clone();
    let callback = call.args[1].clone();
    let paths = collect_param_paths(&callback, param_idents);
    if paths.is_empty() {
        return;
    }
    let bindings = emit_clone_bindings(&paths);
    let mut rewritten = callback;
    substitute_in_expr(&mut rewritten, &paths);
    let func = call.func.clone();

    let new_expr: Expr = syn::parse2(quote! {
        #func(#label, {
            #(#bindings)*
            #rewritten
        })
    })
    .expect("button rewrite produced invalid expr");
    *call = match new_expr {
        Expr::Call(c) => c,
        _ => unreachable!("we just emitted a Call"),
    };
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
