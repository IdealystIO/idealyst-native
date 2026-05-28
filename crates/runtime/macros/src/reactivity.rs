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

        if let Expr::Call(call) = expr {
            if is_button_call(call) && call.args.len() == 2 {
                rewrite_button_args(call, &self.param_idents);
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

/// Rewrites `button(label, on_click)` for reactivity:
///
/// - `label`: if it reads a signal (contains `.get()`), wrap it in a
///   `move || <expr>` closure so the framework's walker installs an
///   Effect that re-evaluates the label on every signal change.
///   Parameter-rooted paths the closure reads are cloned at the
///   closure boundary so the closure is `'static`.
/// - `on_click`: clone any parameter-rooted paths into fresh locals
///   before the closure body so it doesn't borrow.
///
/// Both rewrites are independent; either, both, or neither may
/// fire. The static-label case keeps the original label expression
/// verbatim so `IntoTextSource for &str / String` covers it.
fn rewrite_button_args(call: &mut ExprCall, param_idents: &[String]) {
    let label_orig = call.args[0].clone();
    let callback = call.args[1].clone();

    // --- Label: wrap in reactive closure iff it reads a signal.
    let label_signal_paths = collect_signal_reads(&label_orig);
    let label_expr: Expr = if label_signal_paths.is_empty() {
        label_orig
    } else {
        // Same machinery as rewrite_text_arg: clone every signal +
        // param path at the closure boundary; substitute the
        // rewritten paths in the body; wrap in a `move ||` closure.
        let mut paths = label_signal_paths;
        for extra in collect_param_paths(&label_orig, param_idents) {
            if !paths.contains(&extra) {
                paths.push(extra);
            }
        }
        let bindings = emit_clone_bindings(&paths);
        let mut rewritten = label_orig;
        substitute_in_expr(&mut rewritten, &paths);
        // Coerce to `String` inside the closure so `IntoTextSource`'s
        // closure impl picks it up. Without this, expressions
        // returning `&str` would fail the `Fn() -> String` bound the
        // framework requires for reactive sources.
        syn::parse2(quote! {{
            #(#bindings)*
            move || ::std::string::String::from(#rewritten)
        }})
        .expect("button label reactive rewrite produced invalid expr")
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
