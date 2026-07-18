//! "Why Idealyst over X" comparison pages.
//!
//! Tangent pages — not in the sidebar. Reachable from inline links on
//! the home page (the "How idealyst compares" section) and from each
//! other via the comparisons index. Each individual comparison stands
//! alone; the index here is the hub that lists them all.
//!
//! Tone rule: these pages are not throwing shade at the frameworks
//! they compare against. They exist because the same trade-offs that
//! the author hit in those frameworks are what motivated this one;
//! every comparison should read as "here's the trade-off, here's how
//! idealyst makes the other side of it."

pub mod dioxus;
pub mod electron;
pub mod flutter;
pub mod react;
pub mod web_frameworks;
pub mod when_not;

use runtime_core::{component, ui, Element, Route, StyleApplication};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::pages::common::PageHeader;
use crate::routes::{
    COMPARE_DIOXUS_ROUTE, COMPARE_ELECTRON_ROUTE, COMPARE_FLUTTER_ROUTE, COMPARE_REACT_ROUTE,
    COMPARE_WEB_FRAMEWORKS_ROUTE, COMPARE_WHEN_NOT_ROUTE,
};
use crate::shell::layout;
use crate::styles::{PillarCard, PillarCta, PillarGrid};

/// Comparison index — title, blurb, and a grid of cards pointing to
/// each individual comparison page.
pub fn page() -> Element {
    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "How Idealyst compares",
                blurb = "Idealyst was built to address real friction the author hit using \
                 other UI frameworks. These comparisons aren't throwing shade — every \
                 framework on this list is genuinely good at what it does. They exist to \
                 explain, honestly, why this project takes the trade-offs it does.",
            )
            comparisons_grid()
        }
    };
    layout(content)
}

fn comparisons_grid() -> Element {
    let entries: [(&str, &str, &'static Route<()>); 6] = [
        (
            "Idealyst vs Electron",
            "True native software instead of a Chromium browser wrapped around a \
             webapp. No bundled browser runtime to ship.",
            &COMPARE_ELECTRON_ROUTE,
        ),
        (
            "Idealyst vs React / React Native",
            "No JavaScript runtime shipped with your mobile app, real multithreading, \
             and web performance comparable to React.",
            &COMPARE_REACT_ROUTE,
        ),
        (
            "Idealyst vs Dioxus",
            "Dioxus runs its own renderer everywhere; Idealyst drives the platform's \
             native SDK by default and can also render its own.",
            &COMPARE_DIOXUS_ROUTE,
        ),
        (
            "Idealyst vs Flutter",
            "Same problem, different shape. Idealyst's per-backend architecture means \
             the web target doesn't have to inherit a mobile-first renderer.",
            &COMPARE_FLUTTER_ROUTE,
        ),
        (
            "Idealyst vs Vue, Angular, Svelte",
            "Not a fair comparison — those don't ship to mobile without a wrapper — \
             but web-to-web performance is comparable, and you only need one codebase.",
            &COMPARE_WEB_FRAMEWORKS_ROUTE,
        ),
        (
            "When not to use Idealyst",
            "Rust learning curve, WASM still maturing in places, and a bundle-size \
             story that's heavier than pure JS. Read this before you commit.",
            &COMPARE_WHEN_NOT_ROUTE,
        ),
    ];

    ui! {
        view(style = PillarGrid()) {
            for (title, blurb, route) in entries {
                ComparisonCard(
                    title = title.to_string(),
                    blurb = blurb.to_string(),
                    route = route,
                )
            }
        }
    }
}

/// One card on the comparisons index grid. Promoted to a `#[component]`
/// (rather than a snake_case helper) because it has props and is called
/// from a `for` loop — per the project's component-standards rule
/// (CLAUDE.md §9.5).
///
/// `route` is `Option<&'static Route<()>>` so the props struct can
/// `#[derive(Default)]`; the `ui!` struct-literal coerces a bare
/// `&ROUTE` via `From<T> for Option<T>` at the call site, so authors
/// still write `route = &SOMETHING_ROUTE`. The body unwraps with a
/// clear panic message since a card without a route is meaningless.
#[derive(Default)]
pub struct ComparisonCardProps {
    pub title: String,
    pub blurb: String,
    pub route: Option<&'static Route<()>>,
}

#[component]
pub fn ComparisonCard(props: ComparisonCardProps) -> Element {
    let title = props.title;
    let blurb = props.blurb;
    let route = props
        .route
        .expect("ComparisonCard requires a `route` prop");
    let card_style = PillarCard();
    let cta_style = move || StyleApplication::new(PillarCta::sheet());
    ui! {
        view(style = card_style) {
            Typography(content = title, kind = typography_kind::H3)
            Typography(content = blurb, muted = true)
            link(route = route, params = ()) {
                text(style = cta_style) { "Read \u{2192}" }
            }
        }
    }
}
