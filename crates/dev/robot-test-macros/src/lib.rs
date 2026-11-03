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
//!
//! # Platforms
//!
//! A test that only means something on some platforms says which:
//!
//! ```ignore
//! #[robot_test(platforms = [ios, android])]
//! fn numeric_field_opens_the_number_pad(app: &mut App) { … }
//! ```
//!
//! The names are `web`, `ios`, `android`, `macos`, `linux`, `windows`, and
//! the groups `mobile` (ios, android), `desktop` (macos, linux, windows) and
//! `native` (all but web); `platforms = native` takes one without brackets.
//!
//! Elsewhere such a test never shows as passed. libtest has no way to skip
//! a test at run time, so the platforms go into the test's NAME and its
//! `#[ignore]`: the expansion is a module named after the function holding
//! the `#[test]` `robot_only_<platforms>` (`numeric_field_opens_the_number_pad
//! ::robot_only_ios_android`), ignored with the reason. `idealyst test` reads
//! the names from the binary's `--list`, asks the app its platform, and
//! includes exactly the ignored tests that belong there; the rest are
//! counted `filtered out` — or, when none belong, stay `ignored`. A bare
//! `cargo test` reports them `ignored` with the reason; `cargo test --
//! --include-ignored` runs them and the test itself skips (prints
//! `SKIP`) when the app is on another platform. Because the body sits in a
//! module, it reaches the test file's items through a `use super::*`: write
//! `crate::…` rather than `super::…` / `self::…` paths in such a body.

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

/// The platform names `platforms = …` takes, in the order the test name
/// spells them — the list `robot_test::platforms_of_test` parses a test
/// name with, so the two must stay equal.
const PLATFORMS: [&str; 6] = ["web", "ios", "android", "macos", "linux", "windows"];

/// One `platforms = …` name: a platform or a group of them.
fn platform_names(name: &syn::Ident) -> syn::Result<&'static [&'static str]> {
    Ok(match name.to_string().as_str() {
        "web" => &["web"],
        "ios" => &["ios"],
        "android" => &["android"],
        "macos" => &["macos"],
        "linux" => &["linux"],
        "windows" => &["windows"],
        "mobile" => &["ios", "android"],
        "desktop" => &["macos", "linux", "windows"],
        "native" => &["ios", "android", "macos", "linux", "windows"],
        other => {
            return Err(syn::Error::new_spanned(
                name,
                format!(
                    "unknown platform `{other}`: use web, ios, android, macos, linux, windows, \
                     or the groups mobile, desktop, native"
                ),
            ))
        }
    })
}

/// `platforms = [ios, android]` / `platforms = native` → the platforms, in
/// [`PLATFORMS`] order; `None` for no arguments (runs everywhere).
fn parse_platforms(attr: TokenStream2) -> syn::Result<Option<Vec<&'static str>>> {
    if attr.is_empty() {
        return Ok(None);
    }
    let nv: syn::MetaNameValue = syn::parse2(attr.clone()).map_err(|_| unknown_argument(&attr))?;
    if !nv.path.is_ident("platforms") {
        return Err(unknown_argument(&nv.path));
    }
    let names: Vec<syn::Ident> = match &nv.value {
        syn::Expr::Path(p) => vec![p.path.get_ident().cloned().ok_or_else(|| unknown_argument(p))?],
        syn::Expr::Array(a) => a
            .elems
            .iter()
            .map(|e| match e {
                syn::Expr::Path(p) => p.path.get_ident().cloned().ok_or_else(|| unknown_argument(p)),
                other => Err(unknown_argument(other)),
            })
            .collect::<syn::Result<_>>()?,
        other => return Err(unknown_argument(other)),
    };
    if names.is_empty() {
        return Err(syn::Error::new_spanned(&nv.value, "#[robot_test(platforms = [])] names no platform to run on"));
    }
    let mut wanted = Vec::new();
    for n in &names {
        wanted.extend_from_slice(platform_names(n)?);
    }
    Ok(Some(PLATFORMS.iter().copied().filter(|p| wanted.contains(p)).collect()))
}

fn unknown_argument(at: &impl quote::ToTokens) -> syn::Error {
    syn::Error::new_spanned(
        at,
        "#[robot_test] takes only `platforms = [ios, android]` (or `platforms = native`); declare \
         fixtures as parameters after the app, e.g. `fn my_test(app: &mut App, user: &SignedIn)`",
    )
}

fn expand(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    let platforms = parse_platforms(attr)?;
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

    let acquire = match &platforms {
        None => quote! { ::robot_test::__acquire(#name_str) },
        Some(ps) => quote! { ::robot_test::__acquire_on(#name_str, &[#(#ps),*]) },
    };
    // `__body` carries the author's original signature + block verbatim; the
    // generated `#[test]` decides whether to run or skip, then invokes it.
    let body = quote! {
        fn __body(#inputs) #block

        match #acquire {
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
    };
    let Some(ps) = platforms else {
        return Ok(quote! {
            #[test]
            #(#attrs)*
            #vis fn #name() {
                #body
            }
        });
    };

    // Platform-bound: a module named for the fn, holding the test named
    // for its platforms (see the crate docs, "Platforms").
    let marker = format!("robot_only_{}", ps.join("_"));
    let authored_ignore = attrs.iter().any(|a| a.path().is_ident("ignore"));
    let (leaf, ignore) = if authored_ignore {
        // The author's own `#[ignore]` stands, and the name stops parsing
        // as a platform marker, so the runner never includes it.
        (format_ident!("{marker}_ignored"), quote! {})
    } else {
        let flag = ps.iter().map(|p| format!("--{p}")).collect::<Vec<_>>().join(" / ");
        let reason = format!(
            "runs on {} only: `idealyst test {flag}` runs it (or `cargo test -- --include-ignored` against such an app)",
            ps.join(", ")
        );
        (format_ident!("{marker}"), quote! { #[ignore = #reason] })
    };
    Ok(quote! {
        #vis mod #name {
            #[allow(unused_imports)]
            use super::*;

            #[test]
            #ignore
            #(#attrs)*
            pub fn #leaf() {
                #body
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
    fn unknown_arguments_on_the_attribute_are_an_error() {
        let e = error(quote!(setup = "x"), quote! { fn t(app: &mut App) {} });
        assert!(e.contains("takes only `platforms"), "{e}");
        let e = error(quote!(platforms = [ios, tvos]), quote! { fn t(app: &mut App) {} });
        assert!(e.contains("unknown platform `tvos`"), "{e}");
        let e = error(quote!(platforms = []), quote! { fn t(app: &mut App) {} });
        assert!(e.contains("names no platform"), "{e}");
    }

    fn with_platforms(attr: TokenStream2, item: TokenStream2) -> String {
        expand(attr, item).unwrap().to_string()
    }

    /// A platform-bound test is a module named for the fn holding an
    /// ignored test named for its platforms — the name `idealyst test`
    /// selects by — and checks the app's platform when it runs.
    #[test]
    fn platforms_name_the_test_and_ignore_it_with_the_reason() {
        let out = with_platforms(quote!(platforms = [android, ios]), quote! { fn kb(app: &mut App) {} });
        assert!(out.contains("mod kb"), "{out}");
        assert!(out.contains("use super :: * ;"), "{out}");
        assert!(out.contains("fn robot_only_ios_android ()"), "platforms in canonical order: {out}");
        assert!(out.contains("# [ignore = \"runs on ios, android only"), "{out}");
        assert!(out.contains("__acquire_on (\"kb\" , & [\"ios\" , \"android\"])"), "{out}");
    }

    #[test]
    fn platform_groups_expand() {
        let out = with_platforms(quote!(platforms = native), quote! { fn t(app: &mut App) {} });
        assert!(out.contains("fn robot_only_ios_android_macos_linux_windows ()"), "{out}");
        let out = with_platforms(quote!(platforms = [web, desktop]), quote! { fn t(app: &mut App) {} });
        assert!(out.contains("fn robot_only_web_macos_linux_windows ()"), "{out}");
    }

    /// The author's own `#[ignore]` is kept, not doubled, and the name no
    /// longer parses as a platform marker — so the runner leaves it out.
    #[test]
    fn an_authored_ignore_stands_and_unmarks_the_name() {
        let out = with_platforms(quote!(platforms = [ios]), quote! { #[ignore] fn t(app: &mut App) {} });
        assert_eq!(out.matches("ignore").count(), 2, "one #[ignore] plus the `_ignored` name: {out}");
        assert!(out.contains("fn robot_only_ios_ignored ()"), "{out}");
    }

    /// No platforms: the plain `#[test] fn` it always was.
    #[test]
    fn no_platforms_keeps_the_plain_test() {
        let out = expanded(quote! { fn t(app: &mut App) {} });
        assert!(!out.contains("mod t"), "{out}");
        assert!(!out.contains("ignore"), "{out}");
        assert!(out.contains("__acquire (\"t\")"), "{out}");
    }

    #[test]
    fn no_app_parameter_is_an_error() {
        let e = error(TokenStream2::new(), quote! { fn t() {} });
        assert!(e.contains("app handle first"), "{e}");
    }
}
