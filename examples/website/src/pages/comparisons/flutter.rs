//! "Why Idealyst over Flutter?" — same problem, different shape.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::pages::common::PageHeader;
use crate::routes::{BACKENDS_ROUTE, COMPARISONS_ROUTE, CROSS_PLATFORM_ROUTE};
use crate::shell::layout;

pub fn page() -> Element {
    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Why Idealyst over Flutter?",
                blurb = "Flutter solves a very similar problem: one codebase, every \
                 platform, reactive component model. It does that well. The architectural \
                 choice it made early — own the renderer everywhere — is exactly the \
                 choice Idealyst makes the opposite way.",
            )
            same_goal_different_path()
            web_story()
            architecture_diff()
            footer_links()
        }
    };
    layout(content)
}

fn same_goal_different_path() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Same goal, different bottom layer".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Flutter is mobile-first and ships its own renderer onto every \
                 platform — every Material button, every scroll, every text run is painted \
                 by Skia (now Impeller on iOS) rather than the OS's own widget. That gives \
                 Flutter pixel-identical output across devices but it also means every \
                 platform is the same trade-off: Flutter's text is not the OS's text, \
                 Flutter's scroll physics are Flutter's scroll physics.".to_string(),
            )
            Typography(
                content = "Idealyst's per-backend architecture lets each target use \
                 whatever fits it best. iOS gets UIKit. macOS gets AppKit. Android gets \
                 Android Views. Web gets the DOM. The wgpu backend is there if you want \
                 the fully-rendered path, but it isn't the default for platforms that \
                 already have a perfectly good toolkit.".to_string(),
            )
        }
    }
}

fn web_story() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "The web story".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Flutter's web target had genuine growing pains — first an HTML \
                 renderer, then CanvasKit, now a canvas-based output that paints the whole \
                 app onto a single `<canvas>`. The trade-off is real: text selection, \
                 accessibility, browser scroll, view-source, and SEO all behave \
                 differently than a normal web page because the page isn't really a normal \
                 web page anymore.".to_string(),
            )
            Typography(
                content = "Idealyst's web backend emits actual DOM nodes. `Text` becomes \
                 a `<span>` or `<p>`, `View` becomes a `<div>`, scroll containers use \
                 native scrolling. The same author tree that drives UIKit on iOS produces \
                 a web page that selects, scrolls, indexes, and prints the way the web is \
                 supposed to. The framework also ships an SSR backend for first-paint \
                 HTML and progressive enhancement — see the cross-platform page.".to_string(),
            )
        }
    }
}

fn architecture_diff() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Architectural escape hatch".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Flutter's renderer is part of the framework. If a target needs \
                 something the renderer doesn't do, that target has to wait for the \
                 renderer to grow that capability. Idealyst's Backend trait is the only \
                 seam, and it's a fixed contract — primitives plus styling plus layout \
                 plus refs. New target = new implementation of the trait, rest of the \
                 framework comes along.".to_string(),
            )
        }
    }
}

fn footer_links() -> Element {
    ui! {
        Stack(gap = StackGap::Sm) {
            link(route = &CROSS_PLATFORM_ROUTE, params = ()) {
                Typography(content = "See how cross-platform is structured \u{2192}".to_string())
            }
            link(route = &BACKENDS_ROUTE, params = ()) {
                Typography(content = "Per-backend status \u{2192}".to_string())
            }
            link(route = &COMPARISONS_ROUTE, params = ()) {
                Typography(content = "Back to all comparisons \u{2192}".to_string())
            }
        }
    }
}
