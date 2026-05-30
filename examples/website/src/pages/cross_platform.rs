//! Cross-platform — the "one codebase, native everywhere" feature page.
//! Focuses on the developer-facing promise and the mechanism that makes
//! it true (the Backend trait, real native widgets, convergent
//! behavior). The exhaustive platform list lives on `/targets`; the
//! per-primitive status lives on `/backends` \u{2014} this page links
//! out to both rather than restating them.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{CodePanel, PageHeader, PageSection};
use crate::routes::{BACKENDS_ROUTE, TARGETS_ROUTE};
use crate::shell::{layout_with_toc, TocEntry};

pub fn page() -> Element {
    let one_tree_ref: Ref<ViewHandle> = Ref::new();
    let native_ref: Ref<ViewHandle> = Ref::new();
    let converge_ref: Ref<ViewHandle> = Ref::new();
    let seam_ref: Ref<ViewHandle> = Ref::new();
    let targets_ref: Ref<ViewHandle> = Ref::new();

    let toc = vec![
        TocEntry { handle: one_tree_ref, label: "One author tree" },
        TocEntry { handle: native_ref, label: "Native widgets, not a webview" },
        TocEntry { handle: converge_ref, label: "The same behavior everywhere" },
        TocEntry { handle: seam_ref, label: "The Backend trait is the only seam" },
        TocEntry { handle: targets_ref, label: "See every target" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Truly cross-platform",
                blurb = "The same Rust code renders natively on phones, desktops, the browser, \
                 a GPU surface, even a terminal. Not a fan of the implementation decisions of \
                 a particular platform? Your niche target doesn't have a premade implementation? \
                 Implementing one trait is all it takes to add a new backend and get the rest \
                 of the ecosystem for free.",
            )
            PageSection(handle = one_tree_ref) { one_tree() }
            PageSection(handle = native_ref) { native_widgets() }
            PageSection(handle = converge_ref) { convergent_behavior() }
            PageSection(handle = seam_ref) { backend_seam() }
            PageSection(handle = targets_ref) { see_targets() }
        }
    };
    layout_with_toc(content, toc)
}

// =============================================================================
// Section helpers — heading + prose + optional code panel.
// =============================================================================

fn section(title: &str, paragraphs: Vec<&str>, code: Option<&str>) -> Element {
    let mut children: Vec<Element> = Vec::new();
    let title_text = title.to_string();
    children.push(ui! {
        Typography(content = title_text, kind = idea_ui::typography_kind::H2)
    });
    for p in paragraphs {
        let body = p.to_string();
        children.push(ui! { Typography(content = body) });
    }
    if let Some(src) = code {
        children.push(ui! { CodePanel(src = src) });
    }
    ui! { Stack(gap = StackGap::Lg) { children } }
}

// =============================================================================
// Sections
// =============================================================================

fn one_tree() -> Element {
    let example = "// One component. No `#[cfg(target_os)]`, no platform branches.\n\
                   #[component]\n\
                   fn app() -> Element {\n    \
                       let count = signal!(0);\n    \
                       ui! {\n        \
                           view {\n            \
                               text { format!(\"Taps: {}\", count.get()) }\n            \
                               button(\n                \
                                   label = \"Tap\".to_string(),\n                \
                                   on_click = move || count.update(|n| *n += 1),\n            \
                               )\n        \
                           }\n    \
                       }\n\
                   }\n\
                   \n\
                   // Ship the SAME function to every target:\n\
                   //   idealyst run ios        \u{2192} UIKit\n\
                   //   idealyst run android    \u{2192} Android Views\n\
                   //   idealyst dev --web      \u{2192} WASM + DOM\n\
                   //   idealyst run macos      \u{2192} AppKit";
    section(
        "One author tree",
        vec![
            "You write components against a single vocabulary of primitives \u{2014} \
             `View`, `Text`, `Button`, `ScrollView`, and the rest \u{2014} plus signals \
             for state. That tree knows nothing about the platform it will run on. The \
             CLI handles the per-target build pipeline and wrapper; your code stays \
             platform-agnostic.",
            "There's no \"web version\" and \"mobile version\" of a screen to keep in \
             sync. The branching you'd normally write by hand \u{2014} different \
             components, different layout rules, different event models per platform \
             \u{2014} is absorbed below the primitive layer.",
        ],
        Some(example),
    )
}

fn native_widgets() -> Element {
    section(
        "Native widgets, not a webview",
        vec![
            "A `Button` is a real `UIButton` on iOS, a real Android button view over \
             JNI, an `NSButton` on macOS, and a `<button>` in the DOM. A `ScrollView` \
             is a real `UIScrollView` with native scroll physics and bounce, a real \
             Android scroll container, an `NSScrollView` on macOS. The framework drives \
             the platform's own toolkit \u{2014} it does not ship a renderer that \
             imitates one.",
            "That means the things users feel without thinking about \u{2014} momentum \
             scrolling, text selection, the system back gesture, accessibility focus, \
             keyboard handling \u{2014} are the platform's real implementations, not \
             approximations. The app reads as belonging to the device it's running on.",
            "Where a target has no native toolkit to drive \u{2014} a bare GPU surface, \
             a microcontroller's framebuffer, a terminal grid \u{2014} the framework \
             renders the primitives itself through that backend. Same primitives, \
             different bottom layer.",
        ],
        None,
    )
}

fn convergent_behavior() -> Element {
    section(
        "The same behavior everywhere",
        vec![
            "Backends diverge in mechanism but converge in observable behavior. A scale \
             animation uses `UIView.transform` on iOS, a `CALayer` transform on macOS, \
             and a CSS `transform` on web \u{2014} three different mechanisms, one \
             identical visual result. The Backend trait is where the toolkit \
             differences get absorbed.",
            "This is a deliberate design rule, not an accident: there are no \
             per-platform fudge factors in framework code \u{2014} no \"0.95 scale on \
             iOS but 0.93 on Android because the renders differ.\" If a primitive looks \
             or behaves differently on one backend, that backend is fixed at its root \
             so every target benefits, rather than the call site being patched to paper \
             over it.",
            "The payoff for you: what you verify on the web preview is what ships on \
             the phone. The platform you happen to be developing on isn't special.",
        ],
        None,
    )
}

fn backend_seam() -> Element {
    let example = "// Adding a new platform = implementing one trait.\n\
                   impl Backend for MyBackend {\n    \
                       fn create_view(&mut self, ...) -> NodeId { ... }\n    \
                       fn create_text(&mut self, ...) -> NodeId { ... }\n    \
                       fn insert(&mut self, parent: NodeId, child: NodeId, ...) { ... }\n    \
                       fn apply_style(&mut self, node: NodeId, ...) { ... }\n    \
                       // ...one method per primitive, plus layout / refs / animated values\n\
                   }";
    section(
        "The Backend trait is the only seam",
        vec![
            "Every platform is one implementation of the `Backend` trait. The trait is \
             the framework's single seam to the outside world \u{2014} it knows about \
             primitives (create / update / insert / remove), style application, layout, \
             refs, and animated values, and nothing higher-level. Routing, theming, \
             components, and reactivity all sit above it and work unchanged on any \
             backend that satisfies the contract.",
            "So \"truly cross-platform\" isn't a fixed list of blessed targets. It's an \
             open contract: get the primitive surface right for a new surface \u{2014} a \
             proprietary display, a server-side renderer, a games console \u{2014} and \
             everything the framework already does comes along for free.",
            "Peripheral, platform-specific capabilities (maps, video, web views) don't \
             bloat that core contract either; they plug in as third-party extensions \
             through `Element::External` and a per-backend registry.",
        ],
        Some(example),
    )
}

fn see_targets() -> Element {
    let title = ui! {
        Typography(content = "See every target".to_string(), kind = idea_ui::typography_kind::H2)
    };
    let para = ui! {
        Typography(content = "The full list of platforms idealyst runs on \u{2014} phones, \
            desktops, browsers, GPU surfaces, embedded devices, the terminal \u{2014} lives \
            on the Targets page. The per-primitive implementation status for each backend \
            (what's working, in progress, or planned) lives on the Backends page.".to_string())
    };
    let targets_cta = ui! {
        link(route = &TARGETS_ROUTE, params = ()) {
            Typography(content = "Browse every target \u{2192}".to_string())
        }
    };
    let backends_cta = ui! {
        link(route = &BACKENDS_ROUTE, params = ()) {
            Typography(content = "See the Backends matrix \u{2192}".to_string())
        }
    };
    let children: Vec<Element> = vec![title, para, targets_cta, backends_cta];
    ui! { Stack(gap = StackGap::Md) { children } }
}
