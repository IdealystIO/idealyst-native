//! Home — the marketing landing page.
//!
//! Hero band at the top (headline, subhead, CTA buttons, static
//! sun-glare gradient in the corner), then a quickstart code panel,
//! a four-card pillar grid covering the framework's headline
//! differentiators, and a "platforms" strip listing every supported
//! target.

use std::rc::Rc;

use runtime_core::{component, lazy, ui, Element, IntoElement, Route, StyleApplication};
use idea_ui::Typography;

use crate::components::simulator::{
    Simulator, simulator_placeholder, SimulatorSkin,
};
use crate::pages::common::CodePanel;
use crate::routes::{
    AGENTIC_ROUTE, BACKENDS_ROUTE, COMPARISONS_ROUTE, CONCEPTS_ROUTE, INSTALL_ROUTE,
    QUICKSTART_ROUTE, TARGETS_ROUTE, WHY_RUST_ROUTE,
};
use crate::shell::layout;
use crate::styles::{
    hero_glare_sheet, Hero, HeroCtaRow, HeroHeadline, HeroSubhead, HeroText, HomeSection,
    PillarCard, PillarCta, PillarGrid,
};

pub fn page() -> Element {
    let content = ui! {
        view {
            { hero() }
            { quickstart_section() }
            { pillars_section() }
            { comparisons_section() }
        }
    };
    layout(content)
}

// =============================================================================
// Hero
// =============================================================================

fn hero() -> Element {
    let hero_style = crate::responsive::responsive_style(Hero::sheet());
    let glare_style = hero_glare_sheet();
    let text_style = HeroText();
    let headline_style = crate::responsive::responsive_style(HeroHeadline::sheet());
    let subhead_style = crate::responsive::responsive_style(HeroSubhead::sheet());
    let cta_style = HeroCtaRow();

    let text_children: Vec<Element> = vec![
        ui! { text(style = headline_style) { "One codebase, native everywhere." } },
        ui! {
            text(style = subhead_style) {
                "Idealyst is a reactive UI framework that runs natively on every \
                 target. The platform implementations are extensible by design: use the \
                 ones we ship, or write your own to target anything else."
            }
        },
        ui! {
            view(style = cta_style) {
                link(route = &INSTALL_ROUTE, params = ()) {
                    text { "Install the CLI \u{2192}" }
                }
                link(route = &QUICKSTART_ROUTE, params = ()) {
                    text { "Quickstart" }
                }
            }
        },
    ];
    let text_column = ui! { view(style = text_style) { text_children } };

    // Live preview: an embedded wgpu simulator running the `welcome`
    // scaffold project, with an iOS/Android skin toggle. The same
    // visual proof the headline claims \u{2014} "native everywhere"
    // \u{2014} sits right next to the words.
    let device_column = hero_simulator();

    // Row layout: headline + CTAs on the left, live device on the
    // right. The hero band's overall padding + the row's gap
    // separate the two columns. The glare gradient stays as an
    // absolute-positioned sibling so it can still wash the corner
    // behind both columns.
    let row_style = crate::responsive::responsive_style(crate::styles::HeroRow::sheet());
    ui! {
        view(style = hero_style) {
            view(style = glare_style) {}
            view(style = row_style) {
                text_column
                device_column
            }
        }
    }
}

// =============================================================================
// Hero simulator — the live wgpu preview that sits next to the
// headline. iOS-skinned bezel + canvas, inlined into the hero so the
// headline + device read as one visual unit.
// =============================================================================

fn hero_simulator() -> Element {
    // Wrap the simulator subtree in `lazy! { … }` — on web,
    // wasm-split-cli post-build hoists the body (and its transitive
    // wgpu / welcome / ios_sim deps) into a separate chunk wasm
    // loaded on demand. On native targets the macro is transparent:
    // the body compiles inline and runs synchronously.
    lazy! {
        let build_ui: Rc<dyn Fn() -> Element> = Rc::new(welcome::app);
        ui! {
            view(style = crate::styles::SimulatorStage()) {
                Simulator(
                    build_ui = build_ui,
                    skin = SimulatorSkin::Ios,
                )
            }
        }
    }
    // While the chunk loads, render the device chassis with an "off"
    // screen inside (from `simulator_placeholder`). Reserving the
    // footprint means the surrounding hero layout doesn't reflow
    // when the simulator pops in — the only on-load delta is the
    // wgpu canvas painting INSIDE the chassis.
    .placeholder(|| {
        ui! {
            view(style = crate::styles::SimulatorStage()) {
                { simulator_placeholder(None) }
            }
        }
        .into_element()
    })
    .into_element()
}

// =============================================================================
// Quickstart code panel
// =============================================================================

fn quickstart_section() -> Element {
    let section_style = crate::responsive::responsive_style(HomeSection::sheet());

    let install_snippet =
        "# Install the CLI from the GitHub repo\n\
         cargo install --git https://github.com/IdealystIO/idealyst-native idealyst-cli\n\n\
         # Scaffold a project and run it\n\
         idealyst new my-app\n\
         cd my-app\n\
         idealyst dev          # hot-reload web preview at http://localhost:8080\n\
         idealyst run ios      # build + boot in the iOS simulator\n\
         idealyst run android  # build + install on emulator or device";

    // Vec<Element> children — Typography(...) followed by a brace-block
    // sibling in the same `ui!` scope would otherwise be parsed as
    // children of Body, which doesn't have a `children` field.
    let children: Vec<Element> = vec![
        ui! { Typography(content = "Build an iOS, Web, and Android app in five commands.".to_string(), kind = idea_ui::typography_kind::H2) },
        ui! {
            Typography(
                content = "The same `app()` function runs unchanged on web, iOS, and \
                           Android. The CLI handles the build pipeline and the per-target \
                           wrappers \u{2014} your code stays platform-agnostic.".to_string(),
                muted = true,
            )
        },
        ui! { CodePanel(src = install_snippet) },
    ];

    ui! { view(style = section_style) { children } }
}

// =============================================================================
// Pillars — five headline differentiators. Each card is a teaser
// with a "Read more \u{2192}" footer linking to the page where the
// claim is actually substantiated. Keeps the home page light and
// makes the rest of the site discoverable from a glance.
// =============================================================================

fn pillars_section() -> Element {
    let section_style = crate::responsive::responsive_style(HomeSection::sheet());
    let grid_style = PillarGrid();

    // (title, blurb, destination)
    let pillars: [(&str, &str, &'static Route<()>); 5] = [
        (
            "Truly cross-platform",
            "Idealyst comes with premade platform implementations and is designed to \
             extend to any platform through the Backend Interface.",
            &TARGETS_ROUTE,
        ),
        (
            "Reactive without a virtual DOM",
            "Fine-grained signals mutate exactly the primitives that depend on them. \
             No tree-diffing, no reconciliation, no re-render cycle.",
            &CONCEPTS_ROUTE,
        ),
        (
            "Native-class performance",
            "On every target, idealyst drives the platform's own toolkit directly \u{2014} \
             not a re-rendered abstraction over the top.",
            &BACKENDS_ROUTE,
        ),
        (
            "No bundled runtime",
            "WASM for the web, native binaries everywhere else. No JavaScript engine, no \
             platform VM, no embedded runtime to ship.",
            &WHY_RUST_ROUTE,
        ),
        (
            "AI-forward",
            "Documentation generators and MCP support are built in. As you define \
             components, your LLMs can read live metadata to enrich their context.",
            &AGENTIC_ROUTE,
        ),
    ];

    ui! {
        view(style = section_style) {
            Typography(
                content = "What makes it different".to_string(),
                kind = idea_ui::typography_kind::H2,
            )
            view(style = grid_style) {
                for (title, blurb, route) in pillars {
                    PillarTile(
                        title = title.to_string(),
                        blurb = blurb.to_string(),
                        route = route,
                    )
                }
            }
        }
    }
}

/// One card on the home page's pillar grid. Promoted from the
/// snake_case `pillar_card` helper because it has props and is called
/// from a `for` loop (CLAUDE.md §9.5). Named `PillarTile`, not
/// `PillarCard`, because `PillarCard` is a stylesheet in `styles.rs`
/// — `#[component]` emits `pub type PillarTile = PillarTileProps`
/// which would collide with the stylesheet's type alias.
#[derive(Default)]
pub struct PillarTileProps {
    pub title: String,
    pub blurb: String,
    pub route: Option<&'static Route<()>>,
}

#[component]
pub fn PillarTile(props: PillarTileProps) -> Element {
    let title = props.title;
    let blurb = props.blurb;
    let route = props.route.expect("PillarTile requires a `route` prop");
    let card_style = PillarCard();
    let cta_style = move || StyleApplication::new(PillarCta::sheet());
    ui! {
        view(style = card_style) {
            Typography(content = title, kind = idea_ui::typography_kind::H3)
            Typography(content = blurb, muted = true)
            link(route = route, params = ()) {
                text(style = cta_style) { "Read more \u{2192}" }
            }
        }
    }
}

// =============================================================================
// Comparisons CTA — points to the "Why Idealyst over X" tangent pages
// (Electron / React / Dioxus / Flutter / Vue+Angular+Svelte / when-not-to-use).
// These aren't in the sidebar; this is the primary entry point into them.
// =============================================================================

fn comparisons_section() -> Element {
    let section_style = crate::responsive::responsive_style(HomeSection::sheet());
    let cta_style = move || StyleApplication::new(PillarCta::sheet());
    ui! {
        view(style = section_style) {
            Typography(
                content = "How idealyst compares".to_string(),
                kind = idea_ui::typography_kind::H2,
            )
            Typography(
                content = "If you've shipped real apps in Electron, React Native, Dioxus, \
                 Flutter, or a JS framework, you've probably hit some of the same friction \
                 that motivated this project. Honest framework-by-framework comparisons \
                 (including \"when not to use Idealyst\") live on their own page.".to_string(),
                muted = true,
            )
            link(route = &COMPARISONS_ROUTE, params = ()) {
                text(style = cta_style) { "See the comparisons \u{2192}" }
            }
        }
    }
}

