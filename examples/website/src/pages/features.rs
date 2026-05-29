//! Features overview — the hub for the Features section. A lead
//! paragraph plus a card grid linking to each feature's detail page.
//! Reuses the home page's pillar grid styles so the section reads as a
//! continuation of the landing experience; cards beyond the headline
//! six point at the pages where each adjacent capability already lives
//! (Core concepts, Why Rust, Robot & MCP).

use runtime_core::{ui, Element, Route, StyleApplication};
use idea_ui::Typography;

use crate::routes::{
    AGENTIC_ROUTE, CODE_SPLITTING_ROUTE, CONCEPTS_ROUTE, CROSS_PLATFORM_ROUTE, PERFORMANCE_ROUTE,
    SERVER_FUNCTIONS_ROUTE, SSR_ROUTE, TYPE_SAFETY_ROUTE, WHY_RUST_ROUTE,
};
use crate::shell::layout;
use crate::styles::{HomeSection, PillarCard, PillarCta, PillarGrid};

pub fn page() -> Element {
    let content = ui! {
        View {
            { intro() }
            { grid() }
        }
    };
    layout(content)
}

// =============================================================================
// Intro band — H1 + lead. Mirrors the home page's section padding so the
// overview hub lines up visually with the landing page rather than the
// narrower docs column the detail pages use.
// =============================================================================

fn intro() -> Element {
    let section_style = crate::responsive::responsive_style(HomeSection::sheet());
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Features".to_string(), kind = idea_ui::typography_kind::H1) },
        ui! {
            Typography(
                content = "What you get when one Rust codebase drives every platform: \
                    native output everywhere, a reactive core with no virtual DOM, \
                    full-stack server functions, and a type system that catches whole \
                    classes of bugs before the app runs. Pick a capability to go deep."
                    .to_string(),
                kind = idea_ui::typography_kind::BodyLg,
                muted = true,
            )
        },
    ];
    ui! { View(style = section_style) { children } }
}

// =============================================================================
// Card grid — one teaser per capability. The first six are the headline
// features (each with its own detail page); the last three are adjacent
// differentiators that already have homes elsewhere on the site.
// =============================================================================

fn grid() -> Element {
    let section_style = crate::responsive::responsive_style(HomeSection::sheet());
    let grid_style = PillarGrid();

    // (title, blurb, destination)
    let cards_data: [(&str, &str, &'static Route<()>); 9] = [
        (
            "Truly cross-platform",
            "One `app()` function compiles to native UIKit, Android Views, AppKit, the \
             DOM, a GPU pipeline, even a terminal \u{2014} driving each platform's own \
             toolkit, never a webview.",
            &CROSS_PLATFORM_ROUTE,
        ),
        (
            "High performance",
            "No virtual DOM. Fine-grained signals mutate exactly the primitives that \
             change. Benchmarked head-to-head against React, Vue, and Svelte on \
             identical screens.",
            &PERFORMANCE_ROUTE,
        ),
        (
            "Absolute type safety",
            "The function signature is the contract, end to end. Invalid states don't \
             compile, `match` exhaustiveness is enforced, and refs can't outlive the \
             component that owns them.",
            &TYPE_SAFETY_ROUTE,
        ),
        (
            "Server-side rendering",
            "Render any tree to HTML + CSS at a URL for a fast, SEO-ready first paint, \
             then hand off to the live app by adopting the server-rendered DOM in \
             place.",
            &SSR_ROUTE,
        ),
        (
            "Server functions",
            "Write server logic \u{2014} database queries and all \u{2014} inside your \
             app. The compiler splits it: the server runs the body, the client gets a \
             typed network stub.",
            &SERVER_FUNCTIONS_ROUTE,
        ),
        (
            "Code splitting",
            "The `lazy!` macro carves a UI subtree into a separate wasm chunk that \
             loads on demand. Native targets compile the same block inline.",
            &CODE_SPLITTING_ROUTE,
        ),
        (
            "Reactive without a virtual DOM",
            "Signals are the whole reactive model \u{2014} no tree-diffing, no \
             reconciliation, no re-render cycle. The fundamentals live in Core \
             concepts.",
            &CONCEPTS_ROUTE,
        ),
        (
            "No bundled runtime",
            "WASM on the web, native binaries everywhere else. No JavaScript engine, no \
             platform VM, nothing extra to ship alongside your app.",
            &WHY_RUST_ROUTE,
        ),
        (
            "AI-forward",
            "Documentation generation and MCP support are built in \u{2014} your \
             components expose live metadata that LLMs can read to enrich their \
             context.",
            &AGENTIC_ROUTE,
        ),
    ];

    let mut cards: Vec<Element> = Vec::with_capacity(cards_data.len());
    for (title, blurb, route) in cards_data {
        cards.push(card(title, blurb, route));
    }

    let children: Vec<Element> = vec![ui! { View(style = grid_style) { cards } }];
    ui! { View(style = section_style) { children } }
}

fn card(title: &str, blurb: &str, route: &'static Route<()>) -> Element {
    let card_style = PillarCard();
    let cta_style = move || StyleApplication::new(PillarCta::sheet());
    let title_text = title.to_string();
    let blurb_text = blurb.to_string();
    let children: Vec<Element> = vec![
        ui! { Typography(content = title_text, kind = idea_ui::typography_kind::H3) },
        ui! { Typography(content = blurb_text, muted = true) },
        ui! {
            Link(route = route, params = ()) {
                Text(style = cta_style) { "Read more \u{2192}" }
            }
        },
    ];
    ui! { View(style = card_style) { children } }
}
