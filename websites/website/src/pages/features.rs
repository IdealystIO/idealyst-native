//! Features overview — the hub for the Features section. A lead
//! paragraph plus a card grid linking to each feature's detail page.
//! Reuses the home page's pillar grid styles so the section reads as a
//! continuation of the landing experience; cards beyond the headline
//! six point at the pages where each adjacent capability already lives
//! (Core concepts, Why Rust, Robot & MCP).

use runtime_core::{component, ui, Element, Route, StyleApplication};
use idea_ui::Typography;

use crate::components::Prose;

use crate::routes::{
    AGENTIC_ROUTE, CODE_SPLITTING_ROUTE, CONCEPTS_ROUTE, CROSS_PLATFORM_ROUTE, PERFORMANCE_ROUTE,
    SERVER_FUNCTIONS_ROUTE, SSR_ROUTE, TYPE_SAFETY_ROUTE, WHY_RUST_ROUTE,
};
use crate::shell::layout;
use crate::styles::{HomeSection, PillarCard, PillarCta, PillarGrid};

pub fn page() -> Element {
    let content = ui! {
        view {
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
                    native output everywhere, a reactive core built on signals, \
                    full-stack server functions, and a type system that catches whole \
                    classes of bugs before the app runs."
                    .to_string(),
                kind = idea_ui::typography_kind::BodyLg,
                muted = true,
            )
        },
    ];
    ui! { view(style = section_style) { children } }
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
            "Cross-platform",
            "One `app()` function compiles to native UIKit, Android Views, AppKit, the \
             DOM, a GPU pipeline, even a terminal \u{2014} each backend drives the \
             platform's own toolkit.",
            &CROSS_PLATFORM_ROUTE,
        ),
        (
            "High performance",
            "Every update is a direct write to the platform view that changed. Work \
             stays proportional to the change, however large the tree grows.",
            &PERFORMANCE_ROUTE,
        ),
        (
            "Type safety end to end",
            "The function signature is the contract, from database row to rendered \
             view. Every enum variant is handled \u{2014} `match` exhaustiveness is \
             enforced \u{2014} and the borrow checker ties refs to the component that \
             owns them.",
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
            "Mark a component `#[component(lazy)]` and it ships as a separate wasm \
             chunk that loads on demand. Native targets compile the same body inline.",
            &CODE_SPLITTING_ROUTE,
        ),
        (
            "Fine-grained reactivity",
            "Signals are the whole reactive model: a signal knows every view that \
             reads it and pushes updates straight to them, one primitive at a time \
             \u{2014} no virtual DOM overhead. The fundamentals live in Core concepts.",
            &CONCEPTS_ROUTE,
        ),
        (
            "Ships as compiled code",
            "WASM on the web, native binaries everywhere else. The runtime is Rust \
             code linked into your app, so the download is your app and nothing more.",
            &WHY_RUST_ROUTE,
        ),
        (
            "Built for AI tooling",
            "Documentation generation and MCP support are built in \u{2014} your \
             components expose live metadata that LLMs read to enrich their \
             context.",
            &AGENTIC_ROUTE,
        ),
    ];

    ui! {
        view(style = section_style) {
            view(style = grid_style) {
                for (title, blurb, route) in cards_data {
                    FeatureCard(
                        title = title.to_string(),
                        blurb = blurb.to_string(),
                        route = route,
                    )
                }
            }
        }
    }
}

/// One card on the features grid. Promoted from the snake_case `card`
/// helper because it has props and is called from a `for` loop
/// (CLAUDE.md §9.5). Re-uses the home page's `PillarCard` /
/// `PillarCta` stylesheets so the features grid reads as a
/// continuation of the landing experience.
#[derive(Default)]
pub struct FeatureCardProps {
    pub title: String,
    pub blurb: String,
    pub route: Option<&'static Route<()>>,
}

#[component]
pub fn FeatureCard(props: FeatureCardProps) -> Element {
    let title = props.title;
    let blurb = props.blurb;
    let route = props.route.expect("FeatureCard requires a `route` prop");
    let card_style = PillarCard();
    let cta_style = move || StyleApplication::new(PillarCta::sheet());
    ui! {
        view(style = card_style) {
            Typography(content = title, kind = idea_ui::typography_kind::H3)
            Prose(content = blurb, muted = true)
            link(route = route, params = ()) {
                text(style = cta_style) { "Read more \u{2192}" }
            }
        }
    }
}
