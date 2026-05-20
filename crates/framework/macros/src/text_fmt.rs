//! `text_fmt!` — sugar for constructing a reactive text binding
//! that hands per-fire fan-out to the active backend (e.g. the web
//! backend's JS-side reactive layer).
//!
//! ## Usage
//!
//! ```ignore
//! // `id` is captured (a u32, formatted once at construction);
//! // `global` and `branch` are signals (subscribed to, formatted
//! // per fire by the backend's binding layer).
//! text_fmt!("leaf {}: g={} b={}", id, bind!(global), bind!(branch))
//! ```
//!
//! Args wrapped in `bind!(...)` are treated as signals; bare exprs
//! are captured by-value at construction time and Display-formatted
//! into the binding's static template parts. The macro produces a
//! [`framework_core::TextSource::JsBinding`] complete with:
//!
//! - `template_parts` (N+1 parts surrounding N signal slots, with
//!   captured-arg values pre-formatted into adjacent parts),
//! - `signal_ids`, `initial_values`,
//! - `compute_fallback` — a `Fn() -> String` that re-evaluates the
//!   exact same format expression; used by the walker when the
//!   active backend doesn't support JS bindings.
//!
//! Only plain `{}` placeholders are supported; no `{:?}` /
//! `{:width}` / `{name}` format specs (yet). Using `{{` / `}}` to
//! emit literal braces works.
//!
//! ## Why a proc macro
//!
//! Distinguishing "this arg is a `Signal<T>`, subscribe it" from
//! "this arg is a captured value, bake it" requires per-arg syntactic
//! introspection — declarative `macro_rules!` can't reliably
//! pattern-match `bind!(...)` vs `expr` because both parse as
//! `expr`. The `bind!()` sentinel is a token-level marker the
//! proc macro recognizes.

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprMacro, LitStr, Token};

pub struct TextFmtInput {
    template: LitStr,
    args: Vec<Expr>,
}

impl Parse for TextFmtInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let template: LitStr = input.parse()?;
        let mut args = Vec::new();
        while !input.is_empty() {
            input.parse::<Token![,]>()?;
            if input.is_empty() {
                // Trailing comma — allowed.
                break;
            }
            args.push(input.parse()?);
        }
        Ok(Self { template, args })
    }
}

/// Each macro arg classifies as either:
/// - `Captured(expr)`: bare expression, formatted once at
///   construction time via Display into the adjacent template part.
/// - `Signal(expr)`: wrapped in `bind!(...)`, subscribed to via its
///   `.id()` and recomputed per fire by the backend's binding layer.
enum ArgKind {
    Captured(Expr),
    Signal(Expr),
}

fn classify(arg: &Expr) -> ArgKind {
    if let Expr::Macro(ExprMacro { mac, .. }) = arg {
        // Match the macro by its LAST path segment so users can
        // write either `bind!(g)` or `framework_core::bind!(g)` or
        // `crate::bind!(g)` — all read as the same sentinel.
        let is_bind = mac
            .path
            .segments
            .last()
            .map_or(false, |s| s.ident == "bind");
        if is_bind {
            // `bind!(expr)` — extract the inner expression. If the
            // inner doesn't parse (e.g. `bind!()` with no arg), fall
            // back to treating the whole macro call as captured —
            // the resulting compile error will be clear enough.
            if let Ok(inner) = syn::parse2::<Expr>(mac.tokens.clone()) {
                return ArgKind::Signal(inner);
            }
        }
    }
    ArgKind::Captured(arg.clone())
}

/// Split a format-string template into the static text between
/// each `{}` placeholder. Returns N+1 parts for N placeholders.
/// Only plain `{}` is supported — no format specs. `{{` and `}}`
/// escape to literal `{` / `}`.
fn split_template(s: &str) -> Result<Vec<String>, String> {
    let mut parts = vec![String::new()];
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' => match chars.peek() {
                Some('{') => {
                    chars.next();
                    parts.last_mut().unwrap().push('{');
                }
                Some('}') => {
                    chars.next();
                    parts.push(String::new());
                }
                _ => return Err("text_fmt! only supports plain `{}` placeholders \
                                 (no `{:?}`, `{:width}`, named, or positional specs)".into()),
            },
            '}' => match chars.peek() {
                Some('}') => {
                    chars.next();
                    parts.last_mut().unwrap().push('}');
                }
                _ => return Err("text_fmt!: unmatched `}` in template".into()),
            },
            other => parts.last_mut().unwrap().push(other),
        }
    }
    Ok(parts)
}

pub fn emit(input: TextFmtInput) -> TokenStream2 {
    let template_lit = input.template.clone();
    let template_str = template_lit.value();
    let static_parts = match split_template(&template_str) {
        Ok(p) => p,
        Err(e) => {
            let msg = format!("text_fmt! template error: {}", e);
            return quote! { ::std::compile_error!(#msg) };
        }
    };
    let placeholder_count = static_parts.len() - 1;
    if input.args.len() != placeholder_count {
        let msg = format!(
            "text_fmt!: template has {} `{{}}` placeholders but got {} args",
            placeholder_count,
            input.args.len(),
        );
        return quote! { ::std::compile_error!(#msg) };
    }

    // Classify each arg, give it a generated binding name. Binding
    // each arg into a `let` once means we can use it (a) inside the
    // static-part build code via reference (no consume), AND (b)
    // inside the compute_fallback closure by move at the end —
    // exactly one consume per non-Copy capture. For Copy types
    // (u32, etc. — the typical bench case) all uses are copies.
    let mut classified: Vec<(syn::Ident, ArgKind)> = Vec::with_capacity(input.args.len());
    for (i, arg) in input.args.iter().enumerate() {
        let kind = classify(arg);
        let name = syn::Ident::new(
            &format!("__text_fmt_arg_{}", i),
            proc_macro2::Span::call_site(),
        );
        classified.push((name, kind));
    }

    // Emit per-arg let bindings (binds the arg expr into a stable
    // name we can reference multiple times below).
    let arg_bindings = classified.iter().map(|(name, kind)| match kind {
        ArgKind::Captured(e) => quote! { let #name = #e; },
        ArgKind::Signal(e) => quote! { let #name = #e; },
    });

    // Walk args, building the parts list. Captured args fold into
    // the current accumulating template part; signal args close
    // the current part and start a new one. Final parts length =
    // signal_count + 1.
    let mut parts_segments: Vec<Vec<PartSegment>> = Vec::new();
    let mut current_segs: Vec<PartSegment> = Vec::new();
    current_segs.push(PartSegment::Lit(static_parts[0].clone()));
    let mut signal_names: Vec<syn::Ident> = Vec::new();
    for (i, (name, kind)) in classified.iter().enumerate() {
        let next_static = static_parts[i + 1].clone();
        match kind {
            ArgKind::Captured(_) => {
                current_segs.push(PartSegment::Capture(name.clone()));
                current_segs.push(PartSegment::Lit(next_static));
            }
            ArgKind::Signal(_) => {
                parts_segments.push(std::mem::take(&mut current_segs));
                signal_names.push(name.clone());
                current_segs.push(PartSegment::Lit(next_static));
            }
        }
    }
    parts_segments.push(current_segs);

    // Emit the per-part construction code. Each part is a small
    // String built by concatenating literal segments and
    // Display-formatted captured values.
    let parts_code = parts_segments.iter().map(|segs| {
        let writes = segs.iter().map(|seg| match seg {
            PartSegment::Lit(s) => quote! { __s.push_str(#s); },
            PartSegment::Capture(name) => quote! {
                ::std::write!(&mut __s, "{}", &#name).expect("writing into String can't fail");
            },
        });
        quote! { {
            use ::std::fmt::Write as _;
            let mut __s = ::std::string::String::new();
            #(#writes)*
            __s
        } }
    });

    let signal_id_code = signal_names.iter().map(|n| quote! { (#n).id() });
    let initial_code = signal_names.iter().map(|n| quote! {
        ::framework_core::untrack(|| (#n).get()).to_string()
    });

    // Compute fallback: re-evaluate the exact same format expression
    // on every fire. The closure captures every bound arg by move.
    // For Copy types this is free; for non-Copy each value is moved
    // into the closure (so the prior static-part construction can
    // only reference by `&`, which is what `write!(__s, "{}", &name)`
    // above does — Display takes `&T`).
    let compute_args = classified.iter().map(|(name, kind)| match kind {
        ArgKind::Captured(_) => quote! { #name },
        ArgKind::Signal(_) => quote! { (#name).get() },
    });
    let compute_code = quote! {
        ::std::rc::Rc::new(move || ::std::format!(#template_lit, #(#compute_args),*))
            as ::std::rc::Rc<dyn ::std::ops::Fn() -> ::std::string::String>
    };

    quote! {{
        #(#arg_bindings)*
        ::framework_core::text(::framework_core::TextSource::JsBinding(
            ::framework_core::JsBindingSpec {
                signal_ids: ::std::vec![#(#signal_id_code),*],
                template_parts: ::std::vec![#(#parts_code),*],
                initial_values: ::std::vec![#(#initial_code),*],
                compute_fallback: #compute_code,
            },
        ))
    }}
}

enum PartSegment {
    Lit(String),
    Capture(syn::Ident),
}
