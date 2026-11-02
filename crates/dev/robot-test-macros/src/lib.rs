//! `#[robot_test]` — author a cross-platform end-to-end test as a normal Rust
//! function and have it run like a standard `#[test]`.
//!
//! ```ignore
//! use robot_test::{robot_test, App};
//!
//! #[robot_test]
//! fn increment_updates_count(app: &mut App) {
//!     app.test_id("counter").assert_text("Counter: 0");
//!     app.test_id("inc").click();
//!     app.test_id("inc").click();
//!     app.test_id("counter").assert_text("Counter: 2");
//!     app.signal("count").assert_eq(2);
//! }
//! ```
//!
//! The attribute expands the function to a real `#[test]` whose body acquires a
//! relay-connected [`App`](robot_test::App) and calls the original code with it.
//! Because it's an ordinary test, `cargo test` discovers it. The catch: these
//! tests need a running app + relay, which `cargo test` can't set up on its own
//! — so when no app is reachable the test **skips** (prints a note and returns)
//! instead of failing. `idealyst test` is what prepares the environment
//! (launches the app on the chosen platform, stands up the relay, points the
//! tests at it) and then runs exactly these same tests for real.

//!
//! # Fixtures
//!
//! Parameters after the app handle are **fixtures**: types implementing
//! `robot_test::Fixture`, taken by reference (`&T` or `&mut T`). Each is set
//! up before the body, in parameter order, and torn down after it in reverse
//! order — also when the body (or a later fixture's setup) panics:
//!
//! ```ignore
//! #[robot_test]
//! fn edits_profile(app: &mut App, user: &SignedIn) {
//!     app.test_id("profile-name").assert_text(&user.name);
//! }
//! ```

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, Type};

/// Turn a `fn name(app: &mut App, fixtures…) { … }` into a `#[test] fn name()`
/// that drives the app over the Robot relay. See the crate docs for the full
/// contract.
#[proc_macro_attribute]
pub fn robot_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    match expand(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(
            attr,
            "#[robot_test] takes no arguments; declare fixtures as parameters after the app, \
             e.g. `fn my_test(app: &mut App, user: &SignedIn)`",
        ));
    }
    let func: ItemFn = syn::parse2(item)?;

    // The first argument is the `&mut App` handle the test drives; any after
    // it are fixtures.
    let mut params = func.sig.inputs.iter();
    match params.next() {
        Some(FnArg::Typed(_)) => {}
        Some(receiver @ FnArg::Receiver(_)) => {
            return Err(syn::Error::new_spanned(receiver, "#[robot_test] argument must be a typed `&mut App`, not `self`"))
        }
        None => {
            return Err(syn::Error::new_spanned(
                &func.sig,
                "#[robot_test] functions take the app handle first, e.g. `fn my_test(app: &mut App)`",
            ))
        }
    }
    // (fixture type, whether the body takes it `&mut`)
    let mut fixtures: Vec<(&Type, bool)> = Vec::new();
    for param in params {
        let FnArg::Typed(pat) = param else {
            return Err(syn::Error::new_spanned(param, "#[robot_test] fixtures are typed parameters"));
        };
        match &*pat.ty {
            Type::Reference(r) => fixtures.push((&r.elem, r.mutability.is_some())),
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "#[robot_test] takes a fixture by reference (`&T` or `&mut T`), so it can be torn \
                     down after the test",
                ))
            }
        }
    }

    let name = &func.sig.ident;
    let name_str = name.to_string();
    let vis = &func.vis;
    let attrs = &func.attrs; // keep #[ignore], #[should_panic], doc comments, …
    let inputs = &func.sig.inputs;
    let block = &func.block;

    // `__with_fixture` nests: each fixture's closure sets up the next, the
    // innermost calls the body with all of them.
    let handles: Vec<_> = (0..fixtures.len()).map(|i| format_ident!("__fixture{i}")).collect();
    let passed = fixtures.iter().zip(&handles).map(|((_, is_mut), h)| if *is_mut { quote!(#h) } else { quote!(&*#h) });
    let mut call = quote! { __body(__app, #(#passed),*) };
    for ((ty, _), h) in fixtures.iter().zip(&handles).rev() {
        call = quote! {
            ::robot_test::__with_fixture::<#ty, _>(__app, |__app, #h| #call)
        };
    }

    // `__body` carries the author's original signature + block verbatim; the
    // generated `#[test]` decides whether to run or skip, then invokes it.
    Ok(quote! {
        #[test]
        #(#attrs)*
        #vis fn #name() {
            fn __body(#inputs) #block

            match ::robot_test::__acquire(#name_str) {
                ::robot_test::Acquire::Ready(mut __handle) => {
                    let __app: &mut ::robot_test::App = &mut *__handle;
                    #call;
                }
                ::robot_test::Acquire::Skip(__reason) => {
                    ::std::eprintln!(
                        "[robot_test] SKIP {}: {} — run `idealyst test` (or start `idealyst dev`) to execute it",
                        #name_str, __reason
                    );
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expanded(item: TokenStream2) -> String {
        expand(TokenStream2::new(), item).unwrap().to_string()
    }

    fn error(attr: TokenStream2, item: TokenStream2) -> String {
        expand(attr, item).unwrap_err().to_string()
    }

    #[test]
    fn a_plain_test_calls_the_body_with_the_app() {
        let out = expanded(quote! { fn t(app: &mut App) { app.ping(); } });
        assert!(out.contains("__body (__app ,)") || out.contains("__body (__app)"), "{out}");
        assert!(!out.contains("__with_fixture"), "{out}");
    }

    /// Fixtures nest in parameter order (the first is outermost: set up
    /// first, torn down last), each passed as the body declared it.
    #[test]
    fn fixtures_nest_in_parameter_order_and_keep_their_mutability() {
        let out = expanded(quote! { fn t(app: &mut App, user: &SignedIn, db: &mut Seeded) {} });
        let signed_in = out.find("__with_fixture :: < SignedIn").expect(&out);
        let seeded = out.find("__with_fixture :: < Seeded").expect(&out);
        assert!(signed_in < seeded, "SignedIn wraps Seeded: {out}");
        assert!(out.contains("__body (__app , & * __fixture0 , __fixture1)"), "{out}");
    }

    #[test]
    fn a_fixture_by_value_is_an_error() {
        let e = error(TokenStream2::new(), quote! { fn t(app: &mut App, user: SignedIn) {} });
        assert!(e.contains("by reference"), "{e}");
    }

    #[test]
    fn arguments_on_the_attribute_are_an_error() {
        let e = error(quote!(setup = "x"), quote! { fn t(app: &mut App) {} });
        assert!(e.contains("takes no arguments"), "{e}");
    }

    #[test]
    fn no_app_parameter_is_an_error() {
        let e = error(TokenStream2::new(), quote! { fn t() {} });
        assert!(e.contains("app handle first"), "{e}");
    }
}
