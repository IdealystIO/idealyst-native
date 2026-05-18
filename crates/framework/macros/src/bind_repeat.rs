//! `bind_repeat!` — reactive unbounded list. The macro captures one
//! row's commands as a *template* at snapshot time; the backend's
//! runtime clones the template per row at play time, remapping node
//! ids so each row instance is independent.
//!
//! Author syntax:
//!
//! ```ignore
//! bind_repeat!(count, row = |i| ui! { Text { "▣" } })
//! ```
//!
//! The `count` argument is a *function call* expression — same
//! shape as `bind_when!` / `bind_switch!`. For a simple signal-as-
//! count, write a one-line `#[method]`:
//!
//! ```ignore
//! #[method] fn passthrough(n: i32) -> i32 { n }
//!
//! bind_repeat!(passthrough(count), row = |i| ...)
//! ```
//!
//! Limitations of the template-clone v0:
//! - The row closure runs ONCE at snapshot time (with index 0), so
//!   every cloned row has identical static content. For per-row
//!   content, bind a `#[method]` against the row's signal-derived
//!   data inside the closure rather than reading the closure's `i`
//!   argument directly.
//! - For very large lists the Roku-idiomatic answer is `MarkupGrid`
//!   / `RowList` with cell recycling; lowering to that is a future
//!   phase.

use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprCall, ExprPath, Ident, Token};

pub struct BindRepeatInput {
    call: ExprCall,
    row_builder: Expr,
    style: Option<Expr>,
}

impl Parse for BindRepeatInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let expr: Expr = input.parse()?;
        let call = match expr {
            Expr::Call(c) => c,
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "bind_repeat!(...) expects a function-call expression first, \
                     e.g. `passthrough(count)`",
                ));
            }
        };
        input.parse::<Token![,]>()?;

        // Parse `row = |i| ..., style = expr`. Order-insensitive;
        // trailing comma optional. `style` is what the anchor (the
        // row container) wears — typically a Row-flex stylesheet
        // so the cloned rows lay out horizontally.
        let mut row_builder: Option<Expr> = None;
        let mut style: Option<Expr> = None;

        while !input.is_empty() {
            let key: Ident = input.parse()?;
            input.parse::<Token![=]>()?;
            match key.to_string().as_str() {
                "row" => {
                    row_builder = Some(input.parse()?);
                }
                "style" => {
                    style = Some(input.parse()?);
                }
                other => {
                    return Err(syn::Error::new_spanned(
                        key,
                        format!(
                            "unexpected key `{}` — bind_repeat! accepts `row = |i| ...` \
                             and `style = ...`",
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

        let row_builder = row_builder.ok_or_else(|| {
            syn::Error::new(input.span(), "bind_repeat! requires `row = |i| ...`")
        })?;

        Ok(BindRepeatInput { call, row_builder, style })
    }
}

pub fn emit(input: BindRepeatInput) -> TokenStream2 {
    let BindRepeatInput { call, row_builder, style } = input;

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

    let style_tokens = match style {
        Some(expr) => quote! {
            ::std::option::Option::Some(
                ::framework_core::IntoStyleSource::into_style_source(#expr)
            )
        },
        None => quote! { ::std::option::Option::None },
    };

    quote! {
        {
            // Build the row template once. The closure receives a
            // freshly-allocated `Signal<i32>` (initial value 0) as
            // its `i` parameter — *not* a plain integer — so users
            // can pipe it through `bind!(method(i))` to get
            // per-row dynamic content. At clone time the device
            // runtime allocates a synthetic signal per row, sets
            // its value to the row's index, and substitutes the
            // template's references to this signal id so the
            // bind!s dispatch with the right index.
            //
            // `__invoke_row` is a typed helper that pins the
            // closure's parameter to `Signal<i32>`. Without it
            // Rust can't infer the closure's parameter type from
            // `bind!(method(i))` usages inside the body —
            // inference doesn't propagate through the bind!
            // expansion back out to the closure signature.
            fn __invoke_row<__RowF>(
                builder: __RowF,
                idx: ::framework_core::Signal<i32>,
            ) -> ::framework_core::Primitive
            where
                __RowF: ::std::ops::FnOnce(::framework_core::Signal<i32>)
                    -> ::framework_core::Primitive,
            {
                builder(idx)
            }
            let __row_index: ::framework_core::Signal<i32> =
                ::framework_core::signal!(0i32);
            let __row_index_id: ::std::option::Option<u64> =
                ::std::option::Option::Some(::framework_core::Signal::<i32>::id(&__row_index));
            let __row_template: ::framework_core::Primitive =
                __invoke_row(#row_builder, __row_index);
            ::framework_core::Primitive::Virtualizer {
                item_count: ::framework_core::Derived::<usize> {
                    method:  #method_lit,
                    inputs:  ::std::vec![ #(#id_calls),* ],
                    initial: ::std::vec![ #(#initial_calls),* ],
                    compute: ::std::rc::Rc::new(move || {
                        #func_ident( #(#get_calls),* ) as usize
                    }),
                },
                item_key: ::std::boxed::Box::new(|i| i as u64),
                item_size: ::framework_core::primitives::virtualizer::ItemSize::Known(
                    ::std::rc::Rc::new(|_| 40.0)
                ),
                render_item: ::std::rc::Rc::new(|_i| {
                    // The closure-driven path doesn't have access to
                    // the typed row closure at this scope — generator
                    // backends always use `row_template` instead. For
                    // runtime backends consuming this Virtualizer
                    // shape, the closure-driven row builder needs to
                    // be plumbed (TODO).
                    ::framework_core::Primitive::View {
                        children: ::std::vec::Vec::new(),
                        style: ::std::option::Option::None,
                        ref_fill: ::std::option::Option::None,
                        safe_area_sides: ::framework_core::SafeAreaSides::NONE,
                        #[cfg(feature = "robot")]
                        test_id: ::std::option::Option::None,
                    }
                }),
                row_template:        ::std::option::Option::Some(::std::boxed::Box::new(__row_template)),
                row_index_signal_id: __row_index_id,
                overscan:            1.0,
                horizontal:          false,
                style:               #style_tokens,
                ref_fill:            ::std::option::Option::None,
            }
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
        "bind_repeat!(...) requires a single-segment function name.",
    ))
}
