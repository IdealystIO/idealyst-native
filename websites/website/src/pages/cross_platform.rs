//! Cross-platform — the "one codebase, native everywhere" feature page.
//! Focuses on the developer-facing promise and the mechanism that makes
//! it true (the Backend trait, real native widgets, convergent
//! behavior). The exhaustive platform list lives on `/targets`; the
//! per-primitive status lives on `/backends` \u{2014} this page links
//! out to both rather than restating them.

use runtime_core::{ui, Element, Ref, ViewHandle};
use idea_ui::{Stack, Typography, StackGap};

use crate::pages::common::{PageHeader, PageSection, Section};
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
        TocEntry { handle: native_ref, label: "The platform's own widgets" },
        TocEntry { handle: converge_ref, label: "The same behavior everywhere" },
        TocEntry { handle: seam_ref, label: "The Backend trait is the only seam" },
        TocEntry { handle: targets_ref, label: "See every target" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Cross-platform",
                blurb = "The same Rust code renders natively on phones, desktops, the browser, \
                 a GPU surface, even a terminal. Each platform is one implementation of the \
                 Backend trait \u{2014} implement it for a new target and the whole ecosystem \
                 comes along.",
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
// Sections — each `fn` is a no-param single-call helper that wraps a
// `Section` component invocation. `Section` (in `common.rs`) is the
// PascalCase component that owns the H2 + paragraphs + optional code
// layout (CLAUDE.md §9.5).
// =============================================================================

fn one_tree() -> Element {
    let example = "#[component]\n\
                   fn app() -> Element {\n    \
                       let count = signal(0);\n    \
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
    ui! {
        Section(
            title = "One author tree".to_string(),
            paragraphs = vec![
                "You write components against a single vocabulary of primitives \u{2014} \
                 `view`, `text`, `button`, `scroll_view`, and the rest \u{2014} plus signals \
                 for state. That tree knows nothing about the platform it will run on. The \
                 CLI handles the per-target build pipeline and wrapper; your code stays \
                 platform-agnostic.".to_string(),
                "A screen is written once. Per-platform differences in layout, events, and \
                 rendering are absorbed below the primitive layer, so the tree you author \
                 is the tree every target runs.".to_string(),
            ],
            code = Some(example.to_string()),
        )
    }
}

fn native_widgets() -> Element {
    ui! {
        Section(
            title = "The platform's own widgets".to_string(),
            paragraphs = vec![
                "A `button` is a real `UIButton` on iOS, a real Android button view over \
                 JNI, an `NSButton` on macOS, and a `<button>` in the DOM. A `scroll_view` \
                 is a real `UIScrollView` with native scroll physics and bounce, a real \
                 Android scroll container, an `NSScrollView` on macOS. The framework drives \
                 the platform's own toolkit directly.".to_string(),
                "That means the things users feel without thinking about \u{2014} momentum \
                 scrolling, text selection, the system back gesture, accessibility focus, \
                 keyboard handling \u{2014} are the platform's real implementations. The \
                 app reads as belonging to the device it's running on.".to_string(),
                "Where a target has no native toolkit to drive \u{2014} a bare GPU surface, \
                 a microcontroller's framebuffer, a terminal grid \u{2014} the framework \
                 renders the primitives itself through that backend. Same primitives, \
                 different bottom layer.".to_string(),
            ],
        )
    }
}

fn convergent_behavior() -> Element {
    ui! {
        Section(
            title = "The same behavior everywhere".to_string(),
            paragraphs = vec![
                "Backends diverge in mechanism but converge in observable behavior. A scale \
                 animation uses `UIView.transform` on iOS, a `CALayer` transform on macOS, \
                 and a CSS `transform` on web \u{2014} three different mechanisms, one \
                 identical visual result. The Backend trait is where the toolkit \
                 differences get absorbed.".to_string(),
                "A design rule keeps this true: when a primitive renders differently on \
                 one backend, that backend gets fixed at the root so every target \
                 benefits. The fix lands in the backend implementation, never as a fudge \
                 factor at the call site.".to_string(),
                "The payoff for you: what you verify on the web preview is what ships on \
                 the phone.".to_string(),
            ],
        )
    }
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
    ui! {
        Section(
            title = "The Backend trait is the only seam".to_string(),
            paragraphs = vec![
                "Every platform is one implementation of the `Backend` trait. The trait is \
                 the framework's single seam to the outside world \u{2014} it knows about \
                 primitives (create / update / insert / remove), style application, layout, \
                 refs, and animated values, and nothing higher-level. Routing, theming, \
                 components, and reactivity all sit above it and work unchanged on any \
                 backend that satisfies the contract.".to_string(),
                "Cross-platform here means an open contract: get the primitive surface \
                 right for a new target \u{2014} a proprietary display, a server-side \
                 renderer, a games console \u{2014} and everything the framework already \
                 does comes along for free.".to_string(),
                "Peripheral, platform-specific capabilities (maps, video, web views) plug \
                 in as third-party extensions through `Element::External` and a \
                 per-backend registry, which keeps the core contract small.".to_string(),
            ],
            code = Some(example.to_string()),
        )
    }
}

fn see_targets() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "See every target".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "The full list of platforms idealyst runs on \u{2014} phones, \
                desktops, browsers, GPU surfaces, embedded devices, the terminal \u{2014} lives \
                on the Targets page. The per-primitive implementation status for each backend \
                (what's working, in progress, or planned) lives on the Backends page.".to_string())
            link(route = &TARGETS_ROUTE, params = ()) {
                Typography(content = "Browse every target \u{2192}".to_string())
            }
            link(route = &BACKENDS_ROUTE, params = ()) {
                Typography(content = "See the Backends matrix \u{2192}".to_string())
            }
        }
    }
}
