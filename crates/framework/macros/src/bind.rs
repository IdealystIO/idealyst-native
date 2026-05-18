//! `bind!` — produces a `TextSource::Bound` from a call-shaped
//! expression.
//!
//! The author writes:
//!
//! ```ignore
//! ui! {
//!     Text { bind!(format_count(count)) }
//! }
//! ```
//!
//! …and the macro expands to:
//!
//! ```ignore
//! ::framework_core::TextSource::Bound {
//!     closure:    ::std::boxed::Box::new(move || format_count(count.get())),
//!     signal_ids: vec![ count.id() ],
//!     method:     "format_count",
//! }
//! ```
//!
//! Effect-driven backends (iOS, Android, Web) consume the closure
//! through the walker's existing Reactive path. Backends with
//! declarative wire formats consume `signal_ids` + `method`. The
//! framework itself stays oblivious to which path applies.
//!
//! ## Grammar
//!
//! ```ignore
//! bind!(<ident>(<signal_expr>, <signal_expr>, ...))
//! ```
//!
//! - Single-segment function name (no `Module::path`)
//! - Each argument must be an expression on which `.id()` and
//!   `.get()` both work — typically a `Signal<T>` reference or a
//!   `Copy` `Signal<T>`. (Author lifetimes are their problem; the
//!   macro doesn't insert `&` or `clone()`.)
//! - Zero arguments is legal — produces an empty `signal_ids`
//!   list. Mostly useful as an escape valve.

use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprCall, ExprPath, Token};

pub struct BindInput {
    /// The whole `func(args...)` call expression.
    call: ExprCall,
}

impl Parse for BindInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        // Tolerate a leading `=>` or similar nothing — we just want
        // an Expr that turns out to be a function call.
        let expr: Expr = input.parse()?;
        let call = match expr {
            Expr::Call(c) => c,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "bind!(...) expects a function-call expression like \
                     `format_label(signal_a, signal_b)`",
                ));
            }
        };
        Ok(BindInput { call })
    }
}

pub fn emit(input: BindInput) -> TokenStream2 {
    let call = input.call;

    // Pull the function name. We accept only a single-segment ident
    // path so the name is unambiguous and matches what the consuming
    // backend will see when looking up the transformer. `Module::fn`
    // or `self.method` calls aren't supported in v0 — extract the
    // function into a top-level `#[method]` first.
    let func_ident = match extract_simple_ident(&call.func) {
        Ok(i) => i,
        Err(e) => return e.to_compile_error(),
    };
    let method_lit = syn::LitStr::new(&func_ident.to_string(), func_ident.span());

    // The args appear three times in the expansion:
    //   1. Inside the closure as `arg.get()` for the computed value
    //      path (Effect-driven backends).
    //   2. Outside as `arg.id()` for the binding metadata path
    //      (declarative backends reading signal_ids).
    //   3. Outside as a `serde_json::to_value(&arg.get())` snapshot
    //      so declarative backends can declare each signal's
    //      initial value alongside the binding.
    //
    // Each `arg` token stream is cloned across occurrences so the
    // three call sites have independent copies.
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

    // Wrap the transformer's return value through `format!("{}", _)`
    // so authors can write methods returning any `Display` type
    // (i32, bool, &str, String, etc.) and the closure satisfies
    // `Box<dyn Fn() -> String>`. The Roku side preserves the raw
    // return type — BrightScript's `Label.text = <int>` auto-
    // coerces, so the rendered output matches what the framework's
    // closure produces.
    quote! {
        ::framework_core::TextSource::Bound(
            ::framework_core::Derived::<::std::string::String> {
                method:  #method_lit,
                inputs:  ::std::vec![ #(#id_calls),* ],
                initial: ::std::vec![ #(#initial_calls),* ],
                compute: ::std::rc::Rc::new(move || {
                    ::std::format!("{}", #func_ident( #(#get_calls),* ))
                }),
            }
        )
    }
}

/// Pull the leading identifier out of a `func` expression in a call.
/// Single-segment paths only (`my_fn`, not `crate::my_fn` or
/// `Module::my_fn`) so the symbolic name in the binding is
/// unambiguous.
fn extract_simple_ident(expr: &Expr) -> syn::Result<syn::Ident> {
    if let Expr::Path(ExprPath { qself: None, path, .. }) = expr {
        if path.segments.len() == 1 && path.segments[0].arguments.is_empty() {
            return Ok(path.segments[0].ident.clone());
        }
    }
    Err(syn::Error::new_spanned(
        expr.to_token_stream(),
        "bind!(...) requires a single-segment function name. Lift \
         module-qualified or method calls into a top-level `fn` first.",
    ))
}

// Keep an unused-import shim for `Token` so future grammar
// extensions (e.g. `bind!(name => fn(args))`) don't have to re-add
// the use line. Harmless.
#[allow(dead_code)]
fn _unused_token_keepalive(_: Option<Token![;]>) {}
