//! `bind_when!` — produces a `Primitive::When` with declarative
//! binding metadata, so backends that ship structural reactivity
//! across a wire boundary (Roku) can express conditional subtree
//! rendering without an `Effect`-driven rebuild loop.
//!
//! Author syntax:
//!
//! ```ignore
//! ui! {
//!     View {
//!         bind_when!(is_even(count),
//!             then  = ui! { Text { "Even" } },
//!             else_ = ui! { Text { "Odd"  } },
//!         )
//!     }
//! }
//! ```
//!
//! Where `is_even` is a `#[method]`-tagged transformer returning
//! `bool`, taking signal values as args.
//!
//! Expansion (sketched):
//!
//! ```ignore
//! ::framework_core::Primitive::When {
//!     cond:      Box::new(move || is_even((count).get())),
//!     then:      Box::new(move || /* THEN_EXPR */),
//!     otherwise: Box::new(move || /* ELSE_EXPR */),
//!     style:     None,
//!     binding:   Some(::framework_core::WhenBinding {
//!         signal_ids:     vec![ count.id() ],
//!         cond_method:    "is_even",
//!         initial_values: vec![ /* snapshot of count.get() */ ],
//!     }),
//! }
//! ```
//!
//! Effect-driven backends (iOS/Android/Web) use the closures and
//! rebuild the active branch on signal change. Roku reads the
//! binding via `note_when_binding` and ships both branches +
//! the transformer name across the wire so the device toggles
//! visibility locally.
//!
//! ## Grammar
//!
//! ```ignore
//! bind_when!( <ident>(<signal_expr>*) , then = <expr> , else_ = <expr> )
//! ```
//!
//! - Function name must be a single-segment ident.
//! - Each signal arg supports `.id()` and `.get()`.
//! - `then` and `else_` are required and produce `Primitive` values
//!   (typically a nested `ui! { ... }`).
//! - Trailing comma optional.

use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprCall, ExprPath, Ident, Token};

pub struct BindWhenInput {
    call: ExprCall,
    then_expr: Expr,
    else_expr: Expr,
}

impl Parse for BindWhenInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let expr: Expr = input.parse()?;
        let call = match expr {
            Expr::Call(c) => c,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "bind_when!(...) expects a function-call expression as its first \
                     argument, e.g. `is_even(count)`",
                ));
            }
        };
        input.parse::<Token![,]>()?;

        let mut then_expr: Option<Expr> = None;
        let mut else_expr: Option<Expr> = None;

        // Two `key = value` clauses in any order. Trailing comma is
        // optional. Anything else is an error.
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            let value: Expr = input.parse()?;
            match key.to_string().as_str() {
                "then" => then_expr = Some(value),
                "else_" => else_expr = Some(value),
                other => {
                    return Err(syn::Error::new_spanned(
                        key,
                        format!(
                            "unexpected key `{}` — bind_when! accepts `then = ...` \
                             and `else_ = ...` only",
                            other
                        ),
                    ));
                }
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            } else {
                break;
            }
        }

        let then_expr = then_expr.ok_or_else(|| {
            syn::Error::new(input.span(), "bind_when! requires a `then = ...` clause")
        })?;
        let else_expr = else_expr.ok_or_else(|| {
            syn::Error::new(input.span(), "bind_when! requires an `else_ = ...` clause")
        })?;

        Ok(BindWhenInput { call, then_expr, else_expr })
    }
}

pub fn emit(input: BindWhenInput) -> TokenStream2 {
    let BindWhenInput { call, then_expr, else_expr } = input;

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

    // The `then`/`otherwise` closures need to be `Fn` (callable
    // many times) so the framework's Effect-based path can rebuild
    // on every signal flip. The user's `ui!` invocation can capture
    // signals (which are `Copy`) so this composes naturally.
    quote! {
        ::framework_core::Primitive::When {
            cond:      ::std::boxed::Box::new(move || #func_ident( #(#get_calls),* )),
            then:      ::std::boxed::Box::new(move || #then_expr),
            otherwise: ::std::boxed::Box::new(move || #else_expr),
            style:     ::std::option::Option::None,
            binding:   ::std::option::Option::Some(::framework_core::WhenBinding {
                signal_ids:     ::std::vec![ #(#id_calls),* ],
                cond_method:    #method_lit,
                initial_values: ::std::vec![ #(#initial_calls),* ],
            }),
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
        "bind_when!(...) requires a single-segment function name. \
         Lift module-qualified or method calls into a top-level `fn` first.",
    ))
}
