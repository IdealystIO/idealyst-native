//! Home — the marketing landing page.
//!
//! Hero band at the top (headline, subhead, CTA buttons, static
//! sun-glare gradient in the corner), then a quickstart code panel,
//! a four-card pillar grid covering the framework's headline
//! differentiators, and a "platforms" strip listing every supported
//! target.

use std::rc::Rc;

use runtime_core::{lazy, signal, switch, ui, IntoPrimitive, Primitive, Route, Signal, StyleApplication};
use idea_ui::{tabs, typography, Tab};

use crate::components::simulator::{simulator, SimulatorProps};
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
    let hero_style = crate::responsive::responsive_style(Hero::sheet());
    let glare_style = hero_glare_sheet();
    let text_style = HeroText();
    let headline_style = crate::responsive::responsive_style(HeroHeadline::sheet());
    let subhead_style = crate::responsive::responsive_style(HeroSubhead::sheet());
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
    let text_column = ui! { View(style = text_style) { text_children } };

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
        View(style = hero_style) {
            View(style = glare_style) {}
            View(style = row_style) {
                text_column
                device_column
            }
        }
    }
}

// =============================================================================
// Hero simulator — the live wgpu preview that sits next to the
// headline. iOS/Android tab toggle on top + the bezel-wrapped canvas
// below. Same wiring as the standalone simulator-section pattern,
// just inlined into the hero so the headline + device read as one
// visual unit.
// =============================================================================

fn hero_simulator() -> Primitive {
    // Wrap the entire simulator subtree in `lazy! { … }` — on web,
    // wasm-split-cli post-build hoists the body (and its transitive
    // wgpu / welcome / ios_sim / android_sim deps) into a separate
    // chunk wasm loaded on demand. On native targets the macro is
    // transparent: the body compiles inline and runs synchronously.
    //
    // The tab strip lives inside the lazy block too — it controls
    // the simulator's painter, and the framework's current `lazy!`
    // v1 doesn't support captures across the boundary. A future
    // version can hoist the tab UI out and pass `active` through
    // via wasm-split's shared memory (chunks can read parent-owned
    // signals directly — that's the whole point of wasm-split vs.
    // serde-bridged chunks). For now, the user sees the placeholder
    // briefly while the chunk loads, then the chrome + simulator
    // mount together.
    lazy! {
        let stage_style = crate::styles::SimulatorStage();
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

        // `switch` re-runs the body closure when the tab changes,
        // rebuilding the Simulator with the matching painter. The
        // outgoing Simulator's `on_lost` fires as its Graphics surface
        // tears down so the wgpu host drops cleanly before the new one
        // mounts.
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

        // Outer chassis: black so it blends with the wgpu engine's
        // device_frame_pipeline (which paints opaque black on the canvas
        // outside the screen). See `SimulatorBezel`'s doc.
        let bezel_style = crate::styles::SimulatorBezel();
        let bezel = ui! { View(style = bezel_style) { dynamic_sim } };

        let stage_children: Vec<Primitive> = vec![tab_strip, bezel];
        ui! { View(style = stage_style) { stage_children } }
    }
    // While the chunk loads, render an empty View styled to the
    // stage so the surrounding layout doesn't reflow when the
    // simulator pops in. Recomputing the style fn (it's a cheap
    // const-ish lookup) since the `lazy!` body is a fn item and
    // can't capture from the outer scope.
    .placeholder(|| {
        ui! { View(style = crate::styles::SimulatorStage()) }.into_primitive()
    })
    .into_primitive()
}

// =============================================================================
// Quickstart code panel
// =============================================================================

fn quickstart_section() -> Primitive {
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

    // Vec<Primitive> children — Typography(...) followed by a brace-block
    // sibling in the same `ui!` scope would otherwise be parsed as
    // children of Body, which doesn't have a `children` field.
    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "Build an iOS, Web, and Android app in five commands.".to_string(), kind = idea_ui::typography_kind::H2.into()) },
        ui! {
            Typography(
                content = "The same `app()` function runs unchanged on web, iOS, and \
                           Android. The CLI handles the build pipeline and the per-target \
                           wrappers \u{2014} your code stays platform-agnostic.".to_string(),
                muted = true,
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

    let mut cards: Vec<Primitive> = Vec::with_capacity(pillars.len());
    for (title, blurb, route) in pillars {
        cards.push(pillar_card(title, blurb, route));
    }

    let children: Vec<Primitive> = vec![
        ui! { Typography(content = "What makes it different".to_string(), kind = idea_ui::typography_kind::H2.into()) },
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
        ui! { Typography(content = title_text, kind = idea_ui::typography_kind::H3.into()) },
        ui! { Typography(content = blurb_text, muted = true) },
        ui! {
            Link(route = route, params = ()) {
                Text(style = cta_style) { "Read more \u{2192}" }
            }
        },
    ];
    ui! { View(style = card_style) { children } }
}

