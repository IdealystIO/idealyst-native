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

use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, FnArg, ItemFn};

/// Turn a `fn name(app: &mut App) { … }` into a `#[test] fn name()` that drives
/// the app over the Robot relay. See the crate docs for the full contract.
#[proc_macro_attribute]
pub fn robot_test(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = parse_macro_input!(item as ItemFn);

    // The body must take exactly one argument — the `&mut App` handle the test
    // drives. Anything else is a usage error we surface at compile time.
    if func.sig.inputs.len() != 1 {
        return syn::Error::new_spanned(
            &func.sig,
            "#[robot_test] functions take exactly one argument: the app handle, e.g. `fn my_test(app: &mut App)`",
        )
        .to_compile_error()
        .into();
    }
    if !matches!(func.sig.inputs.first(), Some(FnArg::Typed(_))) {
        return syn::Error::new_spanned(
            &func.sig.inputs,
            "#[robot_test] argument must be a typed `&mut App`, not `self`",
        )
        .to_compile_error()
        .into();
    }

    let name = &func.sig.ident;
    let name_str = name.to_string();
    let vis = &func.vis;
    let attrs = &func.attrs; // keep #[ignore], #[should_panic], doc comments, …
    let inputs = &func.sig.inputs;
    let block = &func.block;

    // `__body` carries the author's original signature + block verbatim; the
    // generated `#[test]` decides whether to run or skip, then invokes it.
    let expanded = quote! {
        #[test]
        #(#attrs)*
        #vis fn #name() {
            fn __body(#inputs) #block

            match ::robot_test::__acquire(#name_str) {
                ::robot_test::Acquire::Ready(mut __handle) => {
                    __body(&mut *__handle);
                }
                ::robot_test::Acquire::Skip(__reason) => {
                    ::std::eprintln!(
                        "[robot_test] SKIP {}: {} — run `idealyst test` (or start `idealyst dev`) to execute it",
                        #name_str, __reason
                    );
                }
            }
        }
    };
    expanded.into()
}
