//! `bind_switch!` — N-way structural reactivity. Same shape as
//! `bind_when!` but with arbitrary literal-pattern arms + a
//! wildcard default.
//!
//! Author syntax:
//!
//! ```ignore
//! bind_switch!(status(count),
//!     "fresh"       => ui! { Text { "🌱 Just started" } },
//!     "warming up"  => ui! { Text { "🔥 Heating"      } },
//!     _             => ui! { Text { "💪 Going strong" } },
//! )
//! ```
//!
//! The macro parses:
//!   - A function-call expression for the discriminant. `status` is
//!     a `#[method]` that takes signal values and returns a value
//!     comparable against the arms (string, int, bool).
//!   - One or more `LITERAL => SUBTREE` arms.
//!   - A required trailing `_ => SUBTREE` default arm.
//!
//! Each arm's subtree is captured in a `Box<dyn Fn() -> Primitive>`
//! so the walker can build it eagerly at snapshot. Roku ships every
//! arm subtree + the default into the wire stream and the device
//! runtime toggles which one is visible on signal change.

use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprCall, ExprLit, ExprPath, Lit, Token};

pub struct BindSwitchInput {
    call: ExprCall,
    arms: Vec<(Lit, Expr)>,
    default: Expr,
}

impl Parse for BindSwitchInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let expr: Expr = input.parse()?;
        let call = match expr {
            Expr::Call(c) => c,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "bind_switch!(...) expects a function-call expression as its first \
                     argument, e.g. `status(count)`",
                ));
            }
        };
        input.parse::<Token![,]>()?;

        let mut arms: Vec<(Lit, Expr)> = Vec::new();
        let mut default: Option<Expr> = None;

        while !input.is_empty() {
            // Either `_ => expr` or `LIT => expr`.
            if input.peek(Token![_]) {
                input.parse::<Token![_]>()?;
                input.parse::<Token![=>]>()?;
                let value: Expr = input.parse()?;
                default = Some(value);
                if input.peek(Token![,]) {
                    input.parse::<Token![,]>()?;
                }
                break;
            }
            let pat: ExprLit = input.parse()?;
            input.parse::<Token![=>]>()?;
            let value: Expr = input.parse()?;
            arms.push((pat.lit, value));
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        let default = default.ok_or_else(|| {
            syn::Error::new(
                input.span(),
                "bind_switch! requires a wildcard `_ => ...` arm as the default",
            )
        })?;

        Ok(BindSwitchInput { call, arms, default })
    }
}

pub fn emit(input: BindSwitchInput) -> TokenStream2 {
    let BindSwitchInput { call, arms, default } = input;

    let func_ident = match extract_simple_ident(&call.func) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error(),
    };
    let method_lit = syn::LitStr::new(&func_ident.to_string(), func_ident.span());

    let arg_exprs: Vec<&Expr> = call.args.iter().collect();
    let id_calls: Vec<TokenStream2> =
        arg_exprs.iter().map(|a| quote! { (#a).id() }).collect();
    let initial_calls: Vec<TokenStream2> = arg_exprs
        .iter()
        .map(|a| {
            quote! {
                ::framework_core::__serde_json::to_value(&(#a).get())
                    .unwrap_or(::framework_core::__serde_json::Value::Null)
            }
        })
        .collect();

    // Per-arm `(serde_json::Value, Box<dyn Fn() -> Primitive>)`
    // pairs. Each pattern literal turns into a `serde_json::Value`
    // for declarative-side equality; each subtree is wrapped in a
    // closure so the walker can build it once at snapshot and so
    // future Effect-driven backends can call it on every change.
    let arm_tokens: Vec<TokenStream2> = arms
        .iter()
        .map(|(pat, body)| {
            let pat_value = lit_to_value_tokens(pat);
            quote! {
                (
                    #pat_value,
                    ::std::boxed::Box::new(move || #body) as ::std::boxed::Box<dyn ::std::ops::Fn() -> ::framework_core::Primitive>,
                )
            }
        })
        .collect();

    quote! {
        ::framework_core::Primitive::SwitchDecl {
            signal_ids:     ::std::vec![ #(#id_calls),* ],
            cond_method:    #method_lit,
            initial_values: ::std::vec![ #(#initial_calls),* ],
            arms:           ::std::vec![ #(#arm_tokens),* ],
            default:        ::std::boxed::Box::new(move || #default),
            style:          ::std::option::Option::None,
        }
    }
}

/// Emit a `serde_json::Value` constructor call for a literal
/// pattern. Supports int, bool, and string literals; floats are
/// allowed but coerced via `f64::from`.
fn lit_to_value_tokens(lit: &Lit) -> TokenStream2 {
    match lit {
        Lit::Int(i) => {
            let i = i.clone();
            quote! { ::framework_core::__serde_json::Value::from(#i) }
        }
        Lit::Bool(b) => {
            let b = b.value;
            quote! { ::framework_core::__serde_json::Value::from(#b) }
        }
        Lit::Str(s) => {
            let s = s.value();
            quote! { ::framework_core::__serde_json::Value::from(#s) }
        }
        Lit::Float(f) => {
            let f = f.clone();
            quote! { ::framework_core::__serde_json::Value::from(#f as f64) }
        }
        other => syn::Error::new_spanned(
            other,
            "bind_switch! arm patterns must be int / bool / string / float literals",
        )
        .to_compile_error(),
    }
}

fn extract_simple_ident(expr: &Expr) -> syn::Result<syn::Ident> {
    if let Expr::Path(ExprPath { qself: None, path, .. }) = expr {
        if path.segments.len() == 1 && path.segments[0].arguments.is_empty() {
            return Ok(path.segments[0].ident.clone());
        }
    }
    Err(syn::Error::new_spanned(
        expr.to_token_stream(),
        "bind_switch!(...) requires a single-segment function name.",
    ))
}
