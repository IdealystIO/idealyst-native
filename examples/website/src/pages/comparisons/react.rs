//! "Why Idealyst over React / React Native?" — the no-JS-runtime pitch.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::pages::common::PageHeader;
use crate::routes::{COMPARISONS_ROUTE, PERFORMANCE_ROUTE, WHY_RUST_ROUTE};
use crate::shell::layout;

pub fn page() -> Element {
    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Why Idealyst over React / React Native?",
                blurb = "React's reactive component model is genuinely a great idea, and \
                 Idealyst owes a lot of its authoring shape to it. Where this project goes \
                 a different direction is on what's running underneath that model on each \
                 platform.",
            )
            no_js_runtime()
            real_threads()
            web_performance()
            same_tree_everywhere()
            footer_links()
        }
    };
    layout(content)
}

fn no_js_runtime() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "No JavaScript runtime on mobile".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "A React Native app ships a JavaScript engine alongside your \
                 code. Every render and every state update crosses the bridge between JS \
                 and the native UI thread. The shape works, but the cost shows up on \
                 cold-start time, on memory, and on touch-to-paint latency once your tree \
                 grows.".to_string(),
            )
            Typography(
                content = "An Idealyst mobile app is a native binary that drives UIKit / \
                 Android Views directly. There's no embedded interpreter to warm up, no \
                 bridge to marshal across, and the framework's reactive system mutates the \
                 platform's view nodes in-place rather than reconciling a virtual tree.".to_string(),
            )
        }
    }
}

fn real_threads() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Real multithreading".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "JavaScript's concurrency model is single-threaded by design. \
                 The new architecture in React Native gives you better scheduling around \
                 that constraint, but background work that can't be punted to a native \
                 module still competes with the UI for the JS thread.".to_string(),
            )
            Typography(
                content = "Rust gives you Send + Sync as part of the type system. Heavy \
                 work — image decoding, parsing, ML inference, anything CPU-bound — moves \
                 onto a real thread with a compiler-checked guarantee that you didn't \
                 race the UI.".to_string(),
            )
        }
    }
}

fn web_performance() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Comparable web performance".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "On the web, Idealyst's fine-grained reactive updates land in \
                 the same performance neighborhood as React's optimized paths, and ahead of \
                 it on tight reactive workloads where a virtual-DOM diff is the bottleneck. \
                 Bundle sizes are higher than a hand-tuned JS app today; see the comparison \
                 with JS-only frameworks and the \"when not to use\" page for the honest \
                 trade-off.".to_string(),
            )
        }
    }
}

fn same_tree_everywhere() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "One tree for web AND native".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "React Native and React (DOM) share a vocabulary but not a \
                 codebase — most teams maintain two parallel apps with shared logic. \
                 Idealyst's `app()` function is the same on web, iOS, Android, and macOS. \
                 The branching that normally happens at the component layer gets absorbed \
                 below the primitive layer instead.".to_string(),
            )
        }
    }
}

fn footer_links() -> Element {
    ui! {
        Stack(gap = StackGap::Sm) {
            link(route = &PERFORMANCE_ROUTE, params = ()) {
                Typography(content = "See the performance page \u{2192}".to_string())
            }
            link(route = &WHY_RUST_ROUTE, params = ()) {
                Typography(content = "Why Rust \u{2192}".to_string())
            }
            link(route = &COMPARISONS_ROUTE, params = ()) {
                Typography(content = "Back to all comparisons \u{2192}".to_string())
            }
        }
    }
}
