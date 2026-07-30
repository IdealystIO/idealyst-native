//! Cross-platform — the "one codebase, native everywhere" feature page.
//! Focuses on the developer-facing promise and the mechanism that makes
//! it true (the platform seam, real native widgets, convergent
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
        TocEntry { handle: one_tree_ref, label: "One component tree" },
        TocEntry { handle: native_ref, label: "Native widgets" },
        TocEntry { handle: converge_ref, label: "Consistent behavior" },
        TocEntry { handle: seam_ref, label: "The platform seam" },
        TocEntry { handle: targets_ref, label: "Targets" },
    ];

    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Cross-platform",
                blurb = "The same Rust code renders natively on phones, desktops, the \
                 browser, a GPU surface, and the terminal. Each platform is one \
                 implementation of the same backend traits.",
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
            title = "One component tree".to_string(),
            paragraphs = vec![
                "Components are written against a fixed set of primitives (`view`, `text`, \
                 `button`, `scroll_view`, and the rest) plus signals for state. The CLI \
                 builds and wraps the same code for each target.".to_string(),
                "Per-platform differences in layout, events, and rendering are handled \
                 below the primitive layer, not in application code.".to_string(),
            ],
            code = Some(example.to_string()),
        )
    }
}

fn native_widgets() -> Element {
    ui! {
        Section(
            title = "Native widgets".to_string(),
            paragraphs = vec![
                "A `button` is a `UIButton` on iOS, an Android button view over JNI, an \
                 `NSButton` on macOS, and a `<button>` in the DOM. A `scroll_view` is a \
                 `UIScrollView`, an Android scroll container, an `NSScrollView`.".to_string(),
                "Scroll physics, text selection, the system back gesture, accessibility \
                 focus, and keyboard handling are the platform's own implementations.".to_string(),
                "Targets without a native toolkit (a bare GPU surface, a terminal grid) \
                 render the primitives through their own drawing layer instead.".to_string(),
            ],
        )
    }
}

fn convergent_behavior() -> Element {
    ui! {
        Section(
            title = "Consistent behavior".to_string(),
            paragraphs = vec![
                "Backends differ in mechanism and converge on the same result. A scale \
                 animation uses `UIView.transform` on iOS, a `CALayer` transform on \
                 macOS, and CSS `transform` on web.".to_string(),
                "When a primitive renders differently on one backend, the fix goes into \
                 that backend. There are no per-platform adjustments in application code \
                 or in the framework's call sites.".to_string(),
            ],
        )
    }
}

fn backend_seam() -> Element {
    let example = "// The structural seam \u{2014} seven operations.\n\
                   impl Host for MyBackend {\n    \
                       type Node = NodeId;\n    \
                       fn insert(&mut self, parent: &mut NodeId, child: NodeId) { ... }\n    \
                       fn remove_child(&mut self, parent: &NodeId, child: &NodeId) { ... }\n    \
                       // ...clear_children, insert_at, create_anchor, supports_splice\n\
                   }\n\
                   \n\
                   // Primitives, style, events \u{2014} one capability trait each.\n\
                   impl caps::TextOps for MyBackend { ... }\n\
                   impl caps::StyleOps for MyBackend { ... }";
    ui! {
        Section(
            title = "The platform seam".to_string(),
            paragraphs = vec![
                "`Host` covers the structural operations \u{2014} how nodes are \
                 parented, reordered and removed. Primitives (create / update), style \
                 application, layout, refs, and animated values arrive as separate \
                 capability traits. Routing, theming, \
                 components, and reactivity sit above them and are backend-independent.".to_string(),
                "Adding a target (a proprietary display, a server-side renderer, a games \
                 console) means implementing that seam and the capabilities the target \
                 supports.".to_string(),
                "Platform-specific capabilities like maps, video, and web views register \
                 as third-party extensions on the scene registry instead of \
                 growing the seam. The Extensibility page covers that mechanism.".to_string(),
            ],
            code = Some(example.to_string()),
        )
    }
}

fn see_targets() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(content = "Targets".to_string(), kind = idea_ui::typography_kind::H2)
            Typography(content = "The full platform list is on the Targets page. \
                Per-primitive implementation status for each backend is on the Backends \
                page.".to_string())
            link(route = &TARGETS_ROUTE, params = ()) {
                Typography(content = "Browse every target \u{2192}".to_string())
            }
            link(route = &BACKENDS_ROUTE, params = ()) {
                Typography(content = "See the Backends matrix \u{2192}".to_string())
            }
        }
    }
}
