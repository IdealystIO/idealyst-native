//! Generates the per-component invocation `macro_rules!` that authors call
//! at the call site, e.g. `counter!(label = "x", value = score)`.
//!
//! When `#[component]` declares no defaults, the generated macro is a
//! trivial wrapper that constructs the props struct and calls the function:
//!
//! ```ignore
//! macro_rules! counter {
//!     ($($name:ident = $value:expr),* $(,)?) => {
//!         counter(&CounterProps { $($name: $value),* })
//!     };
//! }
//! ```
//!
//! When defaults are declared (`#[component(default(step = 1))]`), the
//! generated macro performs TT-munching: a chain of `@__step_i` arms,
//! one per default, each forwarding to a `@__find_NAME` helper that
//! walks the user-provided ident list. If the user provided the field,
//! the default is skipped; otherwise it's filled in.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{Ident, ItemFn};

use crate::component_attr::{ComponentAttr, DefaultEntry};

/// Describes the props parameter shape so the invocation macro can pick
/// between `func(&Type { ... })` and `func(Type { ... })`.
struct PropsType {
    /// Tokens naming the type (e.g. `CardProps`).
    path: TokenStream2,
    /// True when the function takes `props: &Type` (the common case).
    /// False when it takes `props: Type` (owned, used for container
    /// components that need to consume their children).
    by_ref: bool,
}

/// Generates a `macro_rules!` invocation macro for a component, or an
/// empty token stream if the function signature doesn't fit the expected
/// shape (one parameter typed as `&SomeProps` or `SomeProps`).
pub(crate) fn generate_invocation_macro(item_fn: &ItemFn, attr: &ComponentAttr) -> TokenStream2 {
    let fn_name = &item_fn.sig.ident;

    // The invocation macro is named after the fn ident DIRECTLY — no
    // case transform. `ui!` resolves a `Name(...)` call site by emitting
    // `Name!(...)` verbatim, so the macro name must equal the fn name.
    // Components are therefore PascalCase fns (`fn Typography`) whose
    // macro is `Typography!`; the macro BODY calls the real `fn_name`.
    let macro_name = fn_name;

    // No props: emit a thin invocation macro that just calls the fn.
    // Lets `ui!` write `Summary()` (which lowers to `summary!()`) for
    // components that take no arguments.
    if item_fn.sig.inputs.is_empty() {
        return emit_no_args_macro(&macro_name, fn_name);
    }

    let Some(props_type) = props_type_from_sig(&item_fn.sig) else {
        return TokenStream2::new();
    };
    let defaults = &attr.defaults;
    let path = &props_type.path;
    let amp = if props_type.by_ref { quote!(&) } else { quote!() };

    if defaults.is_empty() {
        return emit_trivial_macro(&macro_name, fn_name, &amp, path);
    }
    emit_tt_munching_macro(&macro_name, fn_name, &amp, path, defaults)
}

/// Zero-prop invocation macro: `name!()` (or `name!(children = ...)`
/// if the component accepts children, though no-arg components
/// can't, by definition — we accept the form for parser uniformity).
fn emit_no_args_macro(macro_name: &Ident, fn_name: &Ident) -> TokenStream2 {
    quote! {
        #[allow(unused_macros)]
        macro_rules! #macro_name {
            () => { #fn_name() };
            // Accept (and ignore) a `children = …` clause so the
            // ui! emitter's user-component path doesn't error on
            // child-bearing call sites — though a no-prop component
            // wouldn't have any use for them.
            (children = $children:expr $(,)?) => { #fn_name() };
        }
    }
}

/// Fast path: no defaults, straight pass-through macro.
fn emit_trivial_macro(
    macro_name: &Ident,
    fn_name: &Ident,
    amp: &TokenStream2,
    path: &TokenStream2,
) -> TokenStream2 {
    quote! {
        #[allow(unused_macros)]
        macro_rules! #macro_name {
            ($($name:ident = $value:expr),* $(,)?) => {
                #fn_name(#amp #path {
                    $($name: $value),*
                })
            };
        }
    }
}

/// Defaults path: emit a chained-step macro that walks each declared
/// default and decides whether to insert it.
///
/// Structure: for each default `NAME = EXPR` at index `i`:
///   `@__step_i [user_idents …] [user_fields …] [fill …]`
///       → forwards to `@__find_NAME [remaining = user_idents …]`
///   `@__find_NAME` has 3 arms: head matches NAME (skip), head doesn't
///   match (chop and recurse), remaining empty (insert default into fill).
/// The chain ends at `@__done`, which emits the struct literal.
fn emit_tt_munching_macro(
    macro_name: &Ident,
    fn_name: &Ident,
    amp: &TokenStream2,
    path: &TokenStream2,
    defaults: &[DefaultEntry],
) -> TokenStream2 {
    let n = defaults.len();
    let mut helper_arms: Vec<TokenStream2> = Vec::with_capacity(n * 4);

    for (i, d) in defaults.iter().enumerate() {
        let name = &d.name;
        let expr = &d.expr;
        let find_label = Ident::new(&format!("__find_{}", name), Span::call_site());
        let this_step = Ident::new(&format!("__step_{}", i), Span::call_site());
        let next_step = if i + 1 < n {
            Ident::new(&format!("__step_{}", i + 1), Span::call_site())
        } else {
            Ident::new("__done", Span::call_site())
        };

        // Step entry: kick off the search by forwarding to @__find_NAME.
        helper_arms.push(quote! {
            (@#this_step
                user_idents   [ $($u:ident)* ]
                user_fields   [ $($uf:tt)* ]
                fill          [ $($f:tt)* ]
            ) => {
                #macro_name!(@#find_label
                    remaining     [ $($u)* ]
                    user_idents   [ $($u)* ]
                    user_fields   [ $($uf)* ]
                    fill          [ $($f)* ]
                )
            };
        });

        // Found: head of remaining is literally NAME. Skip the default.
        helper_arms.push(quote! {
            (@#find_label
                remaining     [ #name $($_rest:ident)* ]
                user_idents   [ $($u:ident)* ]
                user_fields   [ $($uf:tt)* ]
                fill          [ $($f:tt)* ]
            ) => {
                #macro_name!(@#next_step
                    user_idents   [ $($u)* ]
                    user_fields   [ $($uf)* ]
                    fill          [ $($f)* ]
                )
            };
        });

        // Not the head: chop one off and keep searching.
        helper_arms.push(quote! {
            (@#find_label
                remaining     [ $_other:ident $($rest:ident)* ]
                user_idents   [ $($u:ident)* ]
                user_fields   [ $($uf:tt)* ]
                fill          [ $($f:tt)* ]
            ) => {
                #macro_name!(@#find_label
                    remaining     [ $($rest)* ]
                    user_idents   [ $($u)* ]
                    user_fields   [ $($uf)* ]
                    fill          [ $($f)* ]
                )
            };
        });

        // Exhausted: user didn't provide it. Fill the default into fill.
        helper_arms.push(quote! {
            (@#find_label
                remaining     [ ]
                user_idents   [ $($u:ident)* ]
                user_fields   [ $($uf:tt)* ]
                fill          [ $($f:tt)* ]
            ) => {
                #macro_name!(@#next_step
                    user_idents   [ $($u)* ]
                    user_fields   [ $($uf)* ]
                    fill          [ $($f)* #name: #expr, ]
                )
            };
        });
    }

    let first_step = Ident::new("__step_0", Span::call_site());

    quote! {
        #[allow(unused_macros)]
        macro_rules! #macro_name {
            // Public entry.
            ($($name:ident = $value:expr),* $(,)?) => {
                #macro_name!(@#first_step
                    user_idents   [ $($name)* ]
                    user_fields   [ $($name: $value,)* ]
                    fill          [ ]
                )
            };

            #(#helper_arms)*

            // Terminal: emit the struct literal by calling the real fn.
            (@__done
                user_idents   [ $($_u:ident)* ]
                user_fields   [ $($uf:tt)* ]
                fill          [ $($f:tt)* ]
            ) => {
                #fn_name(#amp #path {
                    $($uf)*
                    $($f)*
                })
            };
        }
    }
}

/// If the function has exactly one parameter, returns its props type info.
/// Accepts both `_: &T` and `_: T` (where T is a path type).
fn props_type_from_sig(sig: &syn::Signature) -> Option<PropsType> {
    if sig.inputs.len() != 1 {
        return None;
    }
    let syn::FnArg::Typed(pt) = &sig.inputs[0] else {
        return None;
    };
    match &*pt.ty {
        syn::Type::Reference(ref_ty) => {
            let syn::Type::Path(path_ty) = &*ref_ty.elem else {
                return None;
            };
            let path = &path_ty.path;
            Some(PropsType { path: quote! { #path }, by_ref: true })
        }
        syn::Type::Path(path_ty) => {
            let path = &path_ty.path;
            Some(PropsType { path: quote! { #path }, by_ref: false })
        }
        _ => None,
    }
}
