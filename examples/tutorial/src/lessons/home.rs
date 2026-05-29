//! Start-here page: what this tutorial is and how to move through it.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Typography};

use crate::common::{Callout, DocsLink, LessonPage};
use crate::routes::HOME_ROUTE;
use crate::shell;

pub fn page() -> Element {
    shell::layout(ui! {
        LessonPage(
            current = HOME_ROUTE.name(),
            title = "Learn the Idealyst core".to_string(),
            lead = "Reactivity, stylesheets, and media queries \u{2014} taught against \
                runtime-core directly.".to_string(),
        ) {
            Typography(content = "What this is".to_string(), kind = typography_kind::H2)
            Typography(
                content = "A hands-on tour of idealyst-native's core \u{2014} the layer beneath \
                    the component kit. You'll work directly with runtime-core: reactive signals, \
                    the stylesheet system, and responsive breakpoints. No idea-ui components in \
                    the lessons themselves; just the primitives every higher layer is built \
                    from.".to_string()
            )

            Typography(content = "The three tracks".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Reactivity \u{2014} signals, effects, derived state, and batching: \
                    how the framework updates the screen without a virtual DOM or a diff \
                    pass.".to_string()
            )
            Typography(
                content = "Stylesheets \u{2014} style tokens, the stylesheet! macro, variants \
                    and interaction states: how a style is declared once and resolved against \
                    the active theme.".to_string()
            )
            Typography(
                content = "Media queries \u{2014} breakpoint overlays, mobile-first thinking, \
                    and the current_breakpoint signal: one declaration that's responsive on web \
                    (@media) and native (reactive merge) alike.".to_string()
            )

            Typography(content = "How to move through it".to_string(), kind = typography_kind::H2)
            Typography(
                content = "Work top to bottom. Each step is short, ends with a prev/next bar, \
                    and links out to the deep-dive reference docs when you want the full \
                    mechanics. The sidebar tracks where you are.".to_string()
            )

            Callout(label = "Interactive previews are coming".to_string()) {
                Typography(
                    content = "Today the lessons are read-and-understand. Live, editable \
                        previews land later via the companion fiddle \u{2014} the step \
                        structure here is built to host them.".to_string(),
                    muted = true,
                )
            }

            DocsLink(
                summary = "Prefer to read the verbose reference first? Start with the docs \
                    index.".to_string(),
                link_label = "Docs overview".to_string(),
                doc_file = "README.md".to_string(),
            )
        }
    })
}
