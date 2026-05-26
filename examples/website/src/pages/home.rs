//! Home — the marketing landing page.
//!
//! Hero band at the top (headline, subhead, CTA buttons, static
//! sun-glare gradient in the corner), then a quickstart code panel,
//! a four-card pillar grid covering the framework's headline
//! differentiators, and a "platforms" strip listing every supported
//! target.

use runtime_core::{ui, Primitive, Route, StyleApplication};
use idea_ui::{typography, TypographyKind, TypographyTone};

use crate::pages::common::code_panel;
use crate::routes::{
    AGENTIC_ROUTE, BACKENDS_ROUTE, CONCEPTS_ROUTE, INSTALL_ROUTE, QUICKSTART_ROUTE, TARGETS_ROUTE,
    WHY_RUST_ROUTE,
};
use crate::shell::layout;
use crate::styles::{
    hero_glare_sheet, Hero, HeroCtaRow, HeroHeadline, HeroSubhead, HeroText, HomeSection,
    PillarCard, PillarCta, PillarGrid,
};

pub fn page() -> Primitive {
    let content = ui! {
        View {
            { hero() }
            { quickstart_section() }
            { pillars_section() }
        }
    };
    layout(content)
}

// =============================================================================
// Hero
// =============================================================================

fn hero() -> Primitive {
    let hero_style = Hero();
    let glare_style = hero_glare_sheet();
    let text_style = HeroText();
    let headline_style = move || StyleApplication::new(HeroHeadline::sheet());
    let subhead_style = move || StyleApplication::new(HeroSubhead::sheet());
    let cta_style = HeroCtaRow();

    let text_children: Vec<Primitive> = vec![
        ui! { Text(style = headline_style) { "One codebase, native everywhere." } },
        ui! {
            Text(style = subhead_style) {
                "Idealyst is a reactive UI framework that runs as native code on every \
                 target. The platform implementations are extensible by design: use the \
                 ones we ship, or write your own to target anything else."
            }
        },
        ui! {
            View(style = cta_style) {
                Link(route = &INSTALL_ROUTE, params = ()) {
                    Text { "Install the CLI \u{2192}" }
                }
                Link(route = &QUICKSTART_ROUTE, params = ()) {
                    Text { "Quickstart" }
                }
            }
        },
    ];

    ui! {
        View(style = hero_style) {
            View(style = glare_style) {}
            View(style = text_style) { text_children }
        }
    }
}

// =============================================================================
// Quickstart code panel
// =============================================================================

fn quickstart_section() -> Primitive {
    let section_style = HomeSection();

    let install_snippet =
        "# Install the CLI from the GitHub repo\n\
         cargo install --git https://github.com/IdealystIO/idealyst-native idealyst-cli\n\n\
         # Scaffold a project and run it\n\
         idealyst new my-app\n\
         cd my-app\n\
         idealyst dev          # hot-reload web preview at http://localhost:8080\n\
         idealyst run ios      # build + boot in the iOS simulator\n\
         idealyst run android  # build + install on emulator or device";

    // Vec<Primitive> children — Typography(...) followed by a brace-block
    // sibling in the same `ui!` scope would otherwise be parsed as
    // children of Body, which doesn't have a `children` field.
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Build an iOS, Web, and Android app in five commands.".to_string(), kind = TypographyKind::H2) },
        ui! {
            Typography(
                content = "The same `app()` function runs unchanged on web, iOS, and \
                           Android. The CLI handles the build pipeline and the per-target \
                           wrappers \u{2014} your code stays platform-agnostic.".to_string(),
                tone = TypographyTone::Muted,
            )
        },
        code_panel(install_snippet),
    ];

    ui! { View(style = section_style) { children } }
}

// =============================================================================
// Pillars — five headline differentiators. Each card is a teaser
// with a "Read more \u{2192}" footer linking to the page where the
// claim is actually substantiated. Keeps the home page light and
// makes the rest of the site discoverable from a glance.
// =============================================================================

fn pillars_section() -> Primitive {
    let section_style = HomeSection();
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

    let mut cards: Vec<Primitive> = Vec::with_capacity(pillars.len());
    for (title, blurb, route) in pillars {
        cards.push(pillar_card(title, blurb, route));
    }

    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "What makes it different".to_string(), kind = TypographyKind::H2) },
        ui! { View(style = grid_style) { cards } },
    ];

    ui! { View(style = section_style) { children } }
}

fn pillar_card(title: &str, blurb: &str, route: &'static Route<()>) -> Primitive {
    let card_style = PillarCard();
    let cta_style = move || StyleApplication::new(PillarCta::sheet());
    let title_text = title.to_string();
    let blurb_text = blurb.to_string();
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = title_text, kind = TypographyKind::H3) },
        ui! { Typography(content = blurb_text, tone = TypographyTone::Muted) },
        ui! {
            Link(route = route, params = ()) {
                Text(style = cta_style) { "Read more \u{2192}" }
            }
        },
    ];
    ui! { View(style = card_style) { children } }
}
