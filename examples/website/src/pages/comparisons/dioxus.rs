//! "Why Idealyst over Dioxus?" — the native-SDK-by-default pitch, with
//! an explicit credit to the Dioxus team for the tooling Idealyst uses.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::pages::common::PageHeader;
use crate::routes::{BACKENDS_ROUTE, COMPARISONS_ROUTE, FURTHER_READING_ROUTE};
use crate::shell::layout;

pub fn page() -> Element {
    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Why Idealyst over Dioxus?",
                blurb = "Dioxus is the closest project in spirit to Idealyst — a Rust UI \
                 framework targeting every platform, with a reactive component model. The \
                 honest answer to \"why this and not that\" comes down to a different \
                 default about who paints the pixels.",
            )
            credit_where_due()
            rendering_strategy()
            both_options_available()
            footer_links()
        }
    };
    layout(content)
}

fn credit_where_due() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Credit where it's due".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Idealyst uses Subsecond (the Dioxus team's hot-reload tooling) \
                 internally for fast iteration, and Taffy (their flexbox / grid layout \
                 engine) drives layout inside the iOS, Android, and wgpu backends. This \
                 project would have been a much bigger lift without that work. Their \
                 community is worth your attention regardless of which framework you end \
                 up choosing.".to_string(),
            )
        }
    }
}

fn rendering_strategy() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Different rendering strategy".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Dioxus's mobile / desktop targets render through their own \
                 renderer — a step up from wrapping the app in a webview, but still not \
                 the platform's native widget set. UIKit / AppKit / Android Views aren't \
                 in the picture by default.".to_string(),
            )
            Typography(
                content = "Idealyst's default is the opposite: drive the platform's own \
                 toolkit. A `Button` is a real `UIButton`, a real Android button view, a \
                 real `NSButton`. The framework absorbs the per-toolkit differences below \
                 the primitive layer so author code stays portable, but the widget on the \
                 screen is the one the user's OS already knows how to render, animate, and \
                 make accessible.".to_string(),
            )
        }
    }
}

fn both_options_available() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "If you want the GPU-renderer path, Idealyst supports it too".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "There's a genuine case for a fully GPU-rendered app — custom \
                 visual identity, novel widgets that don't map onto native controls, \
                 embedded surfaces with no toolkit at all. Idealyst's wgpu backend is \
                 exactly that path: the same primitives, painted on a GPU surface instead \
                 of into a native view hierarchy. The bundled iPhone- and Android-skin \
                 simulators in the homepage demo are wgpu hosts.".to_string(),
            )
            Typography(
                content = "So the comparison isn't \"Idealyst can't do what Dioxus does\" \
                 — it can. It's that Idealyst's default is the native-SDK path and the \
                 GPU-renderer path is opt-in, whereas Dioxus picks the GPU-renderer path \
                 as the default everywhere.".to_string(),
            )
        }
    }
}

fn footer_links() -> Element {
    ui! {
        Stack(gap = StackGap::Sm) {
            link(route = &BACKENDS_ROUTE, params = ()) {
                Typography(content = "See the Backends page \u{2192}".to_string())
            }
            link(route = &FURTHER_READING_ROUTE, params = ()) {
                Typography(content = "Acknowledgements & further reading \u{2192}".to_string())
            }
            link(route = &COMPARISONS_ROUTE, params = ()) {
                Typography(content = "Back to all comparisons \u{2192}".to_string())
            }
        }
    }
}
