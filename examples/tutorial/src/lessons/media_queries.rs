//! Track 3 — Media queries. Breakpoint overlays, mobile-first, and the
//! current_breakpoint escape hatch.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Typography};

use crate::common::{Callout, CodePanel, DocsLink, LessonPage};
use crate::routes::{MQ_BREAKPOINTS_ROUTE, MQ_MOBILE_FIRST_ROUTE, MQ_SIGNAL_ROUTE};
use crate::shell;

pub fn breakpoints() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = MQ_BREAKPOINTS_ROUTE.name(),
            title = "Breakpoint overlays".to_string(),
            lead = "Declare responsive style with breakpoint blocks in a stylesheet.".to_string(),
        ) {
            Typography(
                content = "A breakpoint block adds rules that apply only once the viewport is \
                    at least a given width. You write the narrowest layout in base, then add or \
                    change properties in breakpoint blocks as the screen widens; the framework \
                    merges the blocks whose width threshold the current viewport has \
                    crossed.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{stylesheet, FlexDirection};

stylesheet! {
    pub Panel<()> {
        base(_t) {                       // Xs — the mobile-first base
            flex_direction: FlexDirection::Column,
            padding: 12.0,
        }
        breakpoint md(_t) {              // >= 768 dp
            flex_direction: FlexDirection::Row,
            padding: 20.0,
        }
        breakpoint lg(_t) { padding: 32.0 }   // >= 1024 dp
    }
}"##.to_string())

            Typography(
                content = "One declaration, two realizations".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "On web, each breakpoint block becomes an @media (min-width: Npx) \
                    rule \u{2014} so even a static or server-rendered first paint is already \
                    responsive, no JS required. On native, the framework reads \
                    current_breakpoint() and merges the active bucket's overlay reactively. \
                    Both use the same thresholds, so a block activates at exactly the same \
                    width everywhere.".to_string()
            )

            Callout(label = "Valid blocks: sm, md, lg, xl".to_string()) {
                Typography(
                    content = "The base block is the xs layout, so there is no breakpoint xs \
                        to write. The macro rejects breakpoint xs with a compile error that \
                        points you back to base.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "How overlays cascade and how each backend realizes them.".to_string(),
                link_label = "Styling reference \u{2014} Responsive breakpoints".to_string(),
                doc_file = "styling.md".to_string(),
            )
        }
    })
}

pub fn mobile_first() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = MQ_MOBILE_FIRST_ROUTE.name(),
            title = "Mobile-first".to_string(),
            lead = "Widen a narrow base; min-width only, by design.".to_string(),
        ) {
            Typography(
                content = "The model is mobile-first and min-width only. There is intentionally \
                    no max-width, no orientation, and no other media features. You write the \
                    narrow layout as base, then add properties as the viewport grows \u{2014} \
                    never the reverse. At a given width, every overlay whose threshold is at or \
                    below the width applies, lowest first, so wider breakpoints win on \
                    conflicts.".to_string()
            )
            Typography(
                content = "Thresholds default to the Tailwind scale \u{2014} sm 640, md 768, lg \
                    1024, xl 1280 dp. Override them once at startup if your design system uses a \
                    different scale.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{install_breakpoints, Breakpoints};

// Call once before mounting (or before the first current_breakpoint read).
install_breakpoints(Breakpoints {
    sm_min: 600.0,
    md_min: 900.0,
    lg_min: 1200.0,
    xl_min: 1600.0,
})
.ok();"##.to_string())

            Callout(label = "Why min-width only".to_string()) {
                Typography(
                    content = "Min-width overlays stack cleanly and stay \
                        cross-platform-consistent. Mixing max-width introduces cascade ambiguity \
                        that's hard to make identical on web and native, so the framework picks \
                        the model that always agrees.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "The bucket definitions, thresholds, and cascade order.".to_string(),
                link_label = "Breakpoint module".to_string(),
                doc_file = "../crates/runtime/core/src/breakpoint.rs".to_string(),
            )
        }
    })
}

pub fn signal_escape() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = MQ_SIGNAL_ROUTE.name(),
            title = "The breakpoint signal".to_string(),
            lead = "current_breakpoint() \u{2014} the imperative escape hatch.".to_string(),
        ) {
            Typography(
                content = "Some layout changes can't be expressed as a style overlay \u{2014} \
                    showing a different set of components at narrow vs. wide widths, say. For \
                    those, read the bucket directly. current_breakpoint() is a memo over the \
                    viewport size; it re-fires only when the bucket changes, so an effect or \
                    conditional that reads it doesn't re-run on every pixel of a \
                    drag-resize.".to_string()
            )
            CodePanel(src = r##"use runtime_core::{current_breakpoint, Breakpoint, Effect};

let _e = Effect::new(|| {
    match current_breakpoint().get() {
        Breakpoint::Xs | Breakpoint::Sm => { /* stacked layout */ }
        _                               => { /* side-by-side layout */ }
    }
});"##.to_string())

            Callout(label = "Prefer declarative blocks".to_string()) {
                Typography(
                    content = "Breakpoint blocks keep web and native in lockstep and survive \
                        server rendering. The signal is the escape hatch for genuinely \
                        imperative switches \u{2014} reach for it only when an overlay can't \
                        express the change.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "viewport_size, the memo, and which backends are wired.".to_string(),
                link_label = "Breakpoint module".to_string(),
                doc_file = "../crates/runtime/core/src/breakpoint.rs".to_string(),
            )
        }
    })
}
