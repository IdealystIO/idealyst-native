//! `bind_press!` — produces a `ButtonAction` (closure + binding
//! metadata) for the `on_click` slot of a `Button`.
//!
//! Author syntax:
//!
//! ```ignore
//! ui! {
//!     Button(
//!         label = "+1",
//!         on_click = bind_press!(increment(count) => count),
//!     )
//! }
//! ```
//!
//! The macro reads:
//!   - The function call shape `increment(count)` — input signals
//!     passed as arguments, function name captured as a string.
//!   - The optional `=> output_signal` clause — where the method's
//!     return value gets written. Omit for fire-and-forget handlers.
//!
//! Expansion populates a `ButtonAction` with:
//!   - `closure`: an `Rc<dyn Fn()>` that, on press, reads each input
//!     signal, calls the named function, and (if an output is set)
//!     writes the result back. Used by Effect-driven backends.
//!   - `binding`: an `ActionBinding` carrying the input signal IDs,
//!     the function name, the optional output signal ID, and a
//!     snapshot of each input's initial value (for backends that
//!     ship signal state across a wire boundary).
//!
//! ## Constraints (mirrors `bind!`):
//!
//! - Function name must be a single-segment identifier (no
//!   `Module::path`).
//! - Each input arg must be an expression that supports both `.id()`
//!   and `.get()` — i.e. a `Signal<T>` or its reference.
//! - The output signal (if present) must also be a `Signal<T>` —
//!   the macro emits `.id()` and `.set(...)` on it.
//! - The method's return type must match the output signal's `T`
//!   (Rust type-checking enforces this at the closure site).

use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprCall, ExprPath, Token};

pub struct BindPressInput {
    call: ExprCall,
    /// `=> output_signal_expr`, optional.
    output: Option<Expr>,
}

impl Parse for BindPressInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let expr: Expr = input.parse()?;
        let call = match expr {
            Expr::Call(c) => c,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "bind_press!(...) expects a function-call expression like \
                     `increment(count)` (optionally followed by `=> output_signal`)",
                ));
            }
        };

        let output = if input.peek(Token![=>]) {
            input.parse::<Token![=>]>()?;
            let expr: Expr = input.parse()?;
            Some(expr)
        } else {
            None
        };

        Ok(BindPressInput { call, output })
    }
}

pub fn emit(input: BindPressInput) -> TokenStream2 {
    let BindPressInput { call, output } = input;

    let func_ident = match extract_simple_ident(&call.func) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error(),
    };
    let method_lit = syn::LitStr::new(&func_ident.to_string(), func_ident.span());

    let arg_exprs: Vec<&Expr> = call.args.iter().collect();
    let get_calls: Vec<TokenStream2> =
        arg_exprs.iter().map(|a| quote! { (#a).get() }).collect();
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

    // The closure does the work on Effect-driven backends. If
    // there's an output signal, it writes the method's return value
    // back into it; otherwise it just calls the method.
    let closure_body = match &output {
        Some(out) => quote! {
            (#out).set(#func_ident( #(#get_calls),* ));
        },
        None => quote! {
            #func_ident( #(#get_calls),* );
        },
    };

    let output_id = match &output {
        Some(out) => quote! { ::std::option::Option::Some((#out).id()) },
        None => quote! { ::std::option::Option::None },
    };

    quote! {
        ::framework_core::Action {
            method: #method_lit,
            inputs: ::std::vec![ #(#id_calls),* ],
            initial: ::std::vec![ #(#initial_calls),* ],
            output: #output_id,
            fire: ::std::rc::Rc::new(move || { #closure_body }),
        }
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
        "bind_press!(...) requires a single-segment function name. \
         Lift module-qualified or method calls into a top-level `fn` first.",
    ))
}
