//! Pins how much code the macros generate per use.
//!
//! Macro expansion and everything downstream of it are redone for a whole crate
//! on every rebuild, so generated volume is a build-time cost nothing else
//! notices: an expansion can double and every other test still passes. Each case
//! measures output minus input, because an expansion echoes its author's tokens
//! and those are not the macro's cost. The unit is whitespace-free characters of
//! the default-feature expansion.
//!
//! A budget sits a little above today's size. Going over it means either trimming
//! the emission or raising the number on purpose in the commit that spends it.

#![cfg(test)]

use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

fn chars(t: &TokenStream2) -> usize {
    t.to_string().chars().filter(|c| !c.is_whitespace()).count()
}

fn check(failures: &mut Vec<String>, label: &str, input: &TokenStream2, output: &TokenStream2, budget: usize) {
    let generated = chars(output).saturating_sub(chars(input));
    println!("{label}: {generated} generated, budget {budget}");
    if generated > budget {
        failures.push(format!("{label}: generated {generated} characters, budget {budget}"));
    }
}

fn component(input: TokenStream2) -> TokenStream2 {
    let parsed = crate::component_attr::parse_component_attr(TokenStream2::new()).expect("valid attr");
    crate::emit_component_tokens(parsed, input, false)
}

fn sheet(input: TokenStream2) -> TokenStream2 {
    let decl: crate::stylesheet::StyleSheetDecl = syn::parse2(input).expect("parse stylesheet");
    crate::stylesheet::emit(decl, 0)
}

// `catalog`, `debug-stats`, `hot-reload`, `strict-docs` and `ui-overlay` each add
// emission of their own, as in `component_golden`.
#[cfg(not(any(
    feature = "catalog",
    feature = "debug-stats",
    feature = "hot-reload",
    feature = "strict-docs",
    feature = "ui-overlay",
)))]
#[test]
fn generated_code_per_use_stays_within_its_budget() {
    let mut failures = Vec::new();

    let zero_arg = quote! { pub fn Screen() -> Element { body() } };
    check(&mut failures, "#[component] zero-arg", &zero_arg, &component(zero_arg.clone()), BUDGET_ZERO_ARG);

    let inline = quote! {
        pub fn Badge(label: String, count: i32, on: bool, tone: Tone) -> Element { body() }
    };
    check(&mut failures, "#[component] four props", &inline, &component(inline.clone()), BUDGET_INLINE);

    let props = quote! {
        pub struct BadgeProps { pub label: String, pub count: i32, pub on: bool, pub tone: Tone }
    };
    check(&mut failures, "#[props] four fields", &props, &crate::props_attr::emit(props.clone()), BUDGET_PROPS);

    let plain = quote! { pub Plain<()> { base(_t) { padding: 8, gap: 4, margin: 2 } } };
    check(&mut failures, "stylesheet! plain", &plain, &sheet(plain.clone()), BUDGET_SHEET_PLAIN);

    let axis = quote! {
        pub Sized<()> {
            base(_t) { padding: 8 }
            variant size { #[default] sm(_t) { padding: 2 } lg(_t) { padding: 4 } }
        }
    };
    check(&mut failures, "stylesheet! one axis", &axis, &sheet(axis.clone()), BUDGET_SHEET_AXIS);

    // Under unit-test spans this includes the IDE recovery shell a real rustc
    // build skips, so it reads high. It is still the same shell every run.
    let call = quote! { Counter(value = score, label = "x") };
    let parsed: runtime_macros_parse::ast::Ui = syn::parse2(call.clone()).expect("parse ui");
    check(&mut failures, "ui! component call", &call, &crate::ui::emit(parsed, &call), BUDGET_UI_CALL);

    assert!(failures.is_empty(), "macro output grew:\n{}", failures.join("\n"));
}

const BUDGET_ZERO_ARG: usize = 1085;
const BUDGET_INLINE: usize = 2360;
const BUDGET_PROPS: usize = 465;
const BUDGET_SHEET_PLAIN: usize = 2095;
const BUDGET_SHEET_AXIS: usize = 8435;
const BUDGET_UI_CALL: usize = 432;
