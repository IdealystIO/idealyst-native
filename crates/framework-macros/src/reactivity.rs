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
/// `button(...)` calls. If the function's declared return type is
/// `Primitive`, also wraps the trailing expression with
/// `IntoPrimitive::into_primitive(...)` so components can return
/// either a bare `Primitive` or a `Bound<H>` (from a primitive
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
/// `Primitive`. We do a simple token-level match — this catches the
/// common spellings (`Primitive`, `framework_core::Primitive`,
/// `::framework_core::Primitive`) without trying to be a full type
/// resolver. Anything else — `Bindable<H>`, `Bound<H>`, `impl Into<…>`,
/// etc. — is left as-is.
fn returns_primitive(item_fn: &ItemFn) -> bool {
    use syn::ReturnType;
    let ty = match &item_fn.sig.output {
        ReturnType::Type(_, ty) => ty,
        ReturnType::Default => return false,
    };
    let rendered = quote::quote!(#ty).to_string();
    // Strip whitespace to normalize `:: framework_core :: Primitive`
    // vs `::framework_core::Primitive`.
    let normalized: String = rendered.chars().filter(|c| !c.is_whitespace()).collect();
    matches!(
        normalized.as_str(),
        "Primitive" | "framework_core::Primitive" | "::framework_core::Primitive"
    )
}

/// Wraps the function's final expression (the implicit return) with
/// `IntoPrimitive::into_primitive(...)`. We only wrap if the body's
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
            ::framework_core::IntoPrimitive::into_primitive(#inner)
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
