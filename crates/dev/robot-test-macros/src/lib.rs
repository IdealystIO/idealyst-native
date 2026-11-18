//! `#[idea_test]` — author a cross-platform end-to-end test as a normal Rust
//! function and have it run like a standard `#[test]`.
//!
//! ```ignore
//! use robot_test::{idea_test, App};
//!
//! #[idea_test]
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
//! #[idea_test]
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
//! #[idea_test(platforms = [ios, android])]
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
//!
//! # Device classes
//!
//! A test that only means something on some kinds of device says which, in
//! the terms the app branches on (see `robot_test::Form`):
//!
//! ```ignore
//! #[idea_test(form = phone)]                 // breakpoint xs / sm
//! #[idea_test(form = [tablet, desktop])]
//! #[idea_test(input = touch)]                // primary pointer is a finger
//! #[idea_test(platforms = [ios], form = tablet)]
//! ```
//!
//! Forms are `phone`, `tablet`, `desktop`; inputs `touch`, `pointer`. The
//! class is checked against what the app reports when the test runs, so a
//! `phone` test runs on an iPhone, a Pixel and a phone-sized browser alike.
//! Where it doesn't match, the test is **not applicable**: it prints
//! `[idea_test] N/A …` with the reason, and `idealyst test` counts it apart
//! from passes.
//!
//! # Suites
//!
//! A test can belong to one or more named suites, so a run can pick a slice
//! of the file (`idealyst test --suite smoke`):
//!
//! ```ignore
//! #[idea_test(suites = smoke)]
//! #[idea_test(suites = [smoke, checkout], platforms = mobile)]
//! ```
//!
//! A suite name is a lowercase identifier (`a`–`z`, digits, single `_`
//! between words). Membership is only a label: a plain `cargo test` or an
//! `idealyst test` without `--suite` runs a suite's tests like any other.
//! Like the platforms, the suites go into the test's name — the runner has
//! only the binary's `--list` — as a path segment `idea_suites_<a>__<b>`
//! (sorted, `__` between names): `checkout_flow::idea_suites_checkout__smoke`,
//! or `…::idea_suites_smoke::robot_only_ios_android` with platforms too.
//! The body sits in a module then, as for platforms: write `crate::…` paths.
//!
//! # `#[robot_test]`
//!
//! The attribute's former name. It expands exactly like `#[idea_test]` and
//! adds a deprecation warning at the call site.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, Type};

/// Turn a `fn name(app: &mut App, fixtures…) { … }` into a `#[test] fn name()`
/// that drives the app over the Robot relay. See the crate docs for the full
/// contract.
#[proc_macro_attribute]
pub fn idea_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    match expand(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

/// Deprecated name of [`macro@idea_test`]: the same expansion, plus a use of
/// a `#[deprecated]` item so the call site gets the warning (`#[deprecated]`
/// on a proc-macro function itself warns nobody).
#[proc_macro_attribute]
pub fn robot_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    match expand(attr.into(), item.into()) {
        Ok(tokens) => quote! {
            #tokens
            const _: () = ::robot_test::__robot_test_is_now_idea_test();
        }
        .into(),
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

/// What `#[idea_test(…)]` was given.
#[derive(Default)]
struct TestArgs {
    /// Suite names, sorted and deduplicated; empty: in no suite.
    suites: Vec<String>,
    /// `None`: every platform.
    platforms: Option<Vec<&'static str>>,
    /// Device forms (`phone`, …); empty: any.
    forms: Vec<String>,
    /// Inputs (`touch`, `pointer`); empty: any.
    inputs: Vec<String>,
}

const FORMS: [&str; 3] = ["phone", "tablet", "desktop"];
const INPUTS: [&str; 2] = ["touch", "pointer"];

/// `name` or `[a, b]` → the identifiers.
fn idents(value: &syn::Expr) -> syn::Result<Vec<syn::Ident>> {
    let names: Vec<syn::Ident> = match value {
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
        return Err(syn::Error::new_spanned(value, "#[idea_test] names nothing here"));
    }
    Ok(names)
}

/// One of `allowed`, or a compile error naming them.
fn one_of(name: &syn::Ident, what: &str, allowed: &[&str]) -> syn::Result<String> {
    let s = name.to_string();
    if allowed.contains(&s.as_str()) {
        Ok(s)
    } else {
        Err(syn::Error::new_spanned(name, format!("unknown {what} `{s}`: use {}", allowed.join(", "))))
    }
}

/// A suite name as the test name can carry it: lowercase words joined by
/// single `_`, since `__` separates the suites in the name.
fn suite_name(name: &syn::Ident) -> syn::Result<String> {
    let s = name.to_string();
    let ok = !s.is_empty()
        && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && !s.starts_with('_')
        && !s.ends_with('_')
        && !s.contains("__");
    if ok {
        Ok(s)
    } else {
        Err(syn::Error::new_spanned(
            name,
            format!("suite name `{s}`: use lowercase words joined by single underscores, e.g. `smoke` or `checkout_flow`"),
        ))
    }
}

/// `suites = …, platforms = …, form = …, input = …`, each optional, in any order.
fn parse_args(attr: TokenStream2) -> syn::Result<TestArgs> {
    let mut out = TestArgs::default();
    if attr.is_empty() {
        return Ok(out);
    }
    let list = syn::parse::Parser::parse2(
        syn::punctuated::Punctuated::<syn::MetaNameValue, syn::Token![,]>::parse_terminated,
        attr.clone(),
    )
    .map_err(|_| unknown_argument(&attr))?;
    for nv in list {
        let names = idents(&nv.value)?;
        if nv.path.is_ident("suites") {
            let mut suites = names.iter().map(suite_name).collect::<syn::Result<Vec<_>>>()?;
            suites.sort();
            suites.dedup();
            out.suites = suites;
        } else if nv.path.is_ident("platforms") {
            let mut wanted = Vec::new();
            for n in &names {
                wanted.extend_from_slice(platform_names(n)?);
            }
            out.platforms = Some(PLATFORMS.iter().copied().filter(|p| wanted.contains(p)).collect());
        } else if nv.path.is_ident("form") {
            out.forms = names.iter().map(|n| one_of(n, "form", &FORMS)).collect::<syn::Result<_>>()?;
        } else if nv.path.is_ident("input") {
            out.inputs = names.iter().map(|n| one_of(n, "input", &INPUTS)).collect::<syn::Result<_>>()?;
        } else {
            return Err(unknown_argument(&nv.path));
        }
    }
    Ok(out)
}

fn unknown_argument(at: &impl quote::ToTokens) -> syn::Error {
    syn::Error::new_spanned(
        at,
        "#[idea_test] takes `suites = [smoke, checkout]`, `platforms = [ios, android]`, \
         `form = [phone, tablet, desktop]` and `input = touch | pointer`; declare fixtures as parameters after the app, e.g. \
         `fn my_test(app: &mut App, user: &SignedIn)`",
    )
}

fn expand(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    let TestArgs { suites, platforms, forms, inputs: input_kinds } = parse_args(attr)?;
    let func: ItemFn = syn::parse2(item)?;

    // The first argument is the `&mut App` handle the test drives; any after
    // it are fixtures.
    let mut params = func.sig.inputs.iter();
    match params.next() {
        Some(FnArg::Typed(_)) => {}
        Some(receiver @ FnArg::Receiver(_)) => {
            return Err(syn::Error::new_spanned(receiver, "#[idea_test] argument must be a typed `&mut App`, not `self`"))
        }
        None => {
            return Err(syn::Error::new_spanned(
                &func.sig,
                "#[idea_test] functions take the app handle first, e.g. `fn my_test(app: &mut App)`",
            ))
        }
    }
    // (fixture type, whether the body takes it `&mut`)
    let mut fixtures: Vec<(&Type, bool)> = Vec::new();
    for param in params {
        let FnArg::Typed(pat) = param else {
            return Err(syn::Error::new_spanned(param, "#[idea_test] fixtures are typed parameters"));
        };
        match &*pat.ty {
            Type::Reference(r) => fixtures.push((&r.elem, r.mutability.is_some())),
            other => {
                return Err(syn::Error::new_spanned(
                    other,
                    "#[idea_test] takes a fixture by reference (`&T` or `&mut T`), so it can be torn \
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
    // A device-class test checks the app's class first; where it doesn't
    // apply it says why and returns (see the crate docs, "Device classes").
    let class_check = if forms.is_empty() && input_kinds.is_empty() {
        quote! {}
    } else {
        let (form_list, input_list) = (forms.iter(), input_kinds.iter());
        quote! {
            if let ::std::option::Option::Some(__why) =
                ::robot_test::__not_applicable(__app, #name_str, &[#(#form_list),*], &[#(#input_list),*])
            {
                ::std::eprintln!("[idea_test] N/A {}: {}", #name_str, __why);
                return;
            }
        }
    };
    // `__body` carries the author's original signature + block verbatim; the
    // generated `#[test]` decides whether to run or skip, then invokes it.
    let body = quote! {
        fn __body(#inputs) #block

        match #acquire {
            ::robot_test::Acquire::Ready(mut __handle) => {
                let __app: &mut ::robot_test::App = &mut *__handle;
                #class_check
                #call;
            }
            ::robot_test::Acquire::Skip(__reason) => {
                ::std::eprintln!(
                    "[idea_test] SKIP {}: {} — run `idealyst test` (or start `idealyst dev`) to execute it",
                    #name_str, __reason
                );
            }
        }
    };
    // `idea_suites_<a>__<b>`: the path segment that names the suites (see
    // the crate docs, "Suites"); the runner parses it back with
    // `robot_test::suites_of_test`.
    let suite_segment = (!suites.is_empty()).then(|| format_ident!("idea_suites_{}", suites.join("__")));

    let (leaf, ignore) = match &platforms {
        None => match &suite_segment {
            None => {
                return Ok(quote! {
                    #[test]
                    #(#attrs)*
                    #vis fn #name() {
                        #body
                    }
                })
            }
            // In suites only: the suite segment is the test itself.
            Some(seg) => (seg.clone(), quote! {}),
        },
        // Platform-bound: the test is named for its platforms (see the
        // crate docs, "Platforms").
        Some(ps) => {
            let marker = format!("robot_only_{}", ps.join("_"));
            if attrs.iter().any(|a| a.path().is_ident("ignore")) {
                // The author's own `#[ignore]` stands, and the name stops
                // parsing as a platform marker, so the runner never includes it.
                (format_ident!("{marker}_ignored"), quote! {})
            } else {
                let flag = ps.iter().map(|p| format!("--{p}")).collect::<Vec<_>>().join(" / ");
                let reason = format!(
                    "runs on {} only: `idealyst test {flag}` runs it (or `cargo test -- --include-ignored` against such an app)",
                    ps.join(", ")
                );
                (format_ident!("{marker}"), quote! { #[ignore = #reason] })
            }
        }
    };
    let mut test = quote! {
        #[test]
        #ignore
        #(#attrs)*
        pub fn #leaf() {
            #body
        }
    };
    // Suites and platforms: the suite segment is a module between the
    // fn's module and the platform-named test.
    if let (Some(seg), Some(_)) = (&suite_segment, &platforms) {
        test = quote! {
            pub mod #seg {
                #[allow(unused_imports)]
                use super::*;

                #test
            }
        };
    }
    Ok(quote! {
        #vis mod #name {
            #[allow(unused_imports)]
            use super::*;

            #test
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
        assert!(e.contains("takes `suites"), "{e}");
        let e = error(quote!(platforms = [ios, tvos]), quote! { fn t(app: &mut App) {} });
        assert!(e.contains("unknown platform `tvos`"), "{e}");
        let e = error(quote!(platforms = []), quote! { fn t(app: &mut App) {} });
        assert!(e.contains("names nothing"), "{e}");
        let e = error(quote!(form = watch), quote! { fn t(app: &mut App) {} });
        assert!(e.contains("unknown form `watch`"), "{e}");
        let e = error(quote!(input = voice), quote! { fn t(app: &mut App) {} });
        assert!(e.contains("unknown input `voice`"), "{e}");
    }

    /// A device-class test checks the app's class before its body and
    /// returns, saying why, where it doesn't apply; a plain test does not.
    #[test]
    fn a_device_class_test_checks_the_app_before_its_body() {
        let out = expand(quote!(form = [phone, tablet], input = touch), quote! { fn t(app: &mut App) {} })
            .unwrap()
            .to_string();
        let check = out.find("__not_applicable").expect(&out);
        assert!(out.contains(r#"& ["phone" , "tablet"]"#) && out.contains(r#"& ["touch"]"#), "{out}");
        assert!(check < out.rfind("__body (__app").unwrap(), "checked before the body: {out}");
        assert!(!expanded(quote! { fn t(app: &mut App) {} }).contains("__not_applicable"));
        // With platforms too: still platform-bound by name.
        let out = expand(quote!(platforms = [ios], form = tablet), quote! { fn t(app: &mut App) {} }).unwrap().to_string();
        assert!(out.contains("robot_only_ios") && out.contains("__not_applicable"), "{out}");
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

    /// Suites alone: a module named for the fn holding a test named for the
    /// suites, sorted and deduplicated — not ignored, since membership only
    /// labels the test.
    #[test]
    fn suites_name_the_test_without_ignoring_it() {
        let out = with_platforms(quote!(suites = [smoke, checkout, smoke]), quote! { fn pay(app: &mut App) {} });
        assert!(out.contains("mod pay"), "{out}");
        assert!(out.contains("pub fn idea_suites_checkout__smoke ()"), "{out}");
        assert!(!out.contains("ignore"), "{out}");
        assert!(out.contains("__acquire (\"pay\")"), "{out}");
        let one = with_platforms(quote!(suites = smoke), quote! { fn t(app: &mut App) {} });
        assert!(one.contains("pub fn idea_suites_smoke ()"), "{one}");
    }

    /// Suites with platforms: the suite segment is a module between the fn's
    /// module and the platform-named test, which stays ignored.
    #[test]
    fn suites_with_platforms_nest_the_platform_test_in_the_suite_module() {
        let out = with_platforms(quote!(platforms = ios, suites = smoke), quote! { fn kb(app: &mut App) {} });
        let module = out.find("pub mod idea_suites_smoke").expect(&out);
        let test = out.find("fn robot_only_ios ()").expect(&out);
        assert!(out.find("mod kb").unwrap() < module && module < test, "{out}");
        assert_eq!(out.matches("use super :: * ;").count(), 2, "{out}");
        assert!(out.contains("# [ignore = \"runs on ios only"), "{out}");
    }

    #[test]
    fn a_malformed_suite_name_is_an_error() {
        for bad in [quote!(suites = Smoke), quote!(suites = [a__b]), quote!(suites = _x), quote!(suites = [])] {
            let e = error(bad.clone(), quote! { fn t(app: &mut App) {} });
            assert!(e.contains("suite name") || e.contains("names nothing"), "{bad}: {e}");
        }
    }

    #[test]
    fn no_app_parameter_is_an_error() {
        let e = error(TokenStream2::new(), quote! { fn t() {} });
        assert!(e.contains("app handle first"), "{e}");
    }
}
