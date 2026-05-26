//! Home — the marketing landing page.
//!
//! Hero band at the top (headline, subhead, CTA buttons, static
//! sun-glare gradient in the corner), then a quickstart code panel,
//! a four-card pillar grid covering the framework's headline
//! differentiators, and a "platforms" strip listing every supported
//! target.

use std::rc::Rc;

use runtime_core::{signal, switch, ui, Primitive, Route, Signal, StyleApplication};
use idea_ui::{tabs, typography, Tab, TypographyKind, TypographyTone};

use crate::components::simulator::{simulator, SimulatorProps};
use crate::pages::common::code_panel;
use crate::routes::{
    AGENTIC_ROUTE, BACKENDS_ROUTE, CONCEPTS_ROUTE, INSTALL_ROUTE, QUICKSTART_ROUTE, TARGETS_ROUTE,
    WHY_RUST_ROUTE,
};
use crate::shell::layout;
use crate::styles::{
    hero_glare_sheet, Hero, HeroCtaRow, HeroHeadline, HeroSubhead, HeroText, HomeSection,
    PillarCard, PillarCta, PillarGrid, SimulatorStage,
};

pub fn page() -> Primitive {
    let content = ui! {
        View {
            { hero() }
            { simulator_section() }
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
// Simulator — embedded wgpu preview of the `welcome` scaffold app,
// with an iOS/Android skin toggle. The framework's `Graphics`
// primitive surfaces a `<canvas>` element on web; `host-web` runs
// the wgpu init + render loop inside it; `IosSim` / `AndroidSim` are
// the two paint policies the toggle flips between.
//
// Skin changes force the entire Simulator subtree to unmount +
// remount via `runtime_core::switch` keyed on the active tab. The
// wgpu host's `Drop` tears the surface + render loop + reactive
// scope back down in order; a fresh Simulator gets built with the
// new skin baked into its on_ready closure. Cleaner than threading
// reactive skin observers through the embedded host stack.
// =============================================================================

fn simulator_section() -> Primitive {
    let section_style = HomeSection();
    let stage_style = SimulatorStage();

    // 0 = iOS, 1 = Android. Lives at the page-scope so the user's
    // selection survives the simulator subtree rebuild.
    let active: Signal<usize> = signal!(0_usize);
    let on_change: Rc<dyn Fn(usize)> = Rc::new(move |idx| active.set(idx));

    let tab_strip = ui! {
        Tabs(
            tabs = vec![
                Tab { label: "iOS".to_string() },
                Tab { label: "Android".to_string() },
            ],
            active = active,
            on_change = on_change,
        )
    };

    // `switch` re-runs the body closure when `active` changes,
    // rebuilding the Simulator with the matching painter. The
    // outgoing Simulator's `on_lost` fires as its Graphics surface
    // tears down \u{2014} the wgpu host drops cleanly before the new
    // one mounts.
    let dynamic_sim = switch(
        move || active.get(),
        |&idx| {
            let build_ui: Rc<dyn Fn() -> Primitive> = Rc::new(welcome::app);
            #[cfg(target_arch = "wasm32")]
            let skin: Option<Rc<dyn host_web::Painter>> = match idx {
                1 => Some(Rc::new(android_sim::AndroidSim::new())),
                _ => Some(Rc::new(ios_sim::IosSim::new())),
            };
            #[cfg(not(target_arch = "wasm32"))]
            let skin = {
                // The Simulator's wgpu path is web-only; on native
                // targets the prop is unused (the Graphics surface
                // still allocates but nothing drives it).
                let _ = idx;
                None
            };
            ui! {
                Simulator(
                    build_ui = build_ui,
                    skin = skin,
                )
            }
        },
    );

    let stage_children: Vec<Primitive> = vec![tab_strip, dynamic_sim];

    let header_children: Vec<Primitive> = vec![
        ui! {
            Typography(
                content = "See it running.".to_string(),
                kind = TypographyKind::H2,
            )
        },
        ui! {
            Typography(
                content = "The same app, two skins. The embedded preview is the \
                    `welcome` scaffold project rendered through the wgpu backend with \
                    a UIKit or Material-3 paint policy \u{2014} switch between them \
                    and the entire surface rebuilds against the new skin.".to_string(),
                tone = TypographyTone::Muted,
            )
        },
        ui! { View(style = stage_style) { stage_children } },
    ];

    ui! { View(style = section_style) { header_children } }
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
