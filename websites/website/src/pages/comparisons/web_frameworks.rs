//! "Why Idealyst over Vue / Angular / Svelte?" — the not-fair-comparison
//! page. They're not cross-platform; the honest pitch is one-codebase-
//! everywhere with web performance that holds up.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::pages::common::PageHeader;
use crate::routes::{COMPARISONS_ROUTE, COMPARE_WHEN_NOT_ROUTE, PERFORMANCE_ROUTE};
use crate::shell::layout;

const GITHUB_DISCUSSIONS_URL: &str =
    "https://github.com/IdealystIO/idealyst-native/discussions";

pub fn page() -> Element {
    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "Why Idealyst over Vue, Angular, Svelte?",
                blurb = "Vue, Angular, and Svelte are excellent at what they were built \
                 for: building web apps. The honest answer up front is that any \
                 head-to-head with idealyst isn't quite a fair comparison, because they \
                 weren't designed to ship to mobile or desktop without an Electron- or \
                 Capacitor-style wrapper.",
            )
            not_a_fair_comparison()
            web_performance()
            bundle_size_caveat()
            footer_links()
        }
    };
    layout(content)
}

fn not_a_fair_comparison() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "What you'd actually be choosing".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "If you only ship to the browser, a mature JS framework is \
                 probably the right answer today. The ecosystem is enormous, every hire \
                 already knows it, the bundle is small, and the tooling is battle-tested. \
                 Idealyst's win here is not \"better web framework\" — it's \"same \
                 codebase also runs natively on iOS, Android, macOS, and a GPU surface.\" \
                 If that second half is part of your roadmap, the trade-off shifts.".to_string(),
            )
        }
    }
}

fn web_performance() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Performance on the web".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Web-to-web, Idealyst's fine-grained reactive system lands in \
                 the same neighborhood as the fastest JS frameworks and ahead of the \
                 slower ones on tight reactive workloads. Signals mutate exactly the DOM \
                 nodes that depend on them — no virtual-DOM diff, no top-down \
                 component-tree pass to invalidate.".to_string(),
            )
        }
    }
}

fn bundle_size_caveat() -> Element {
    let github_text = "If you've thought about WASM bundle-size optimization and have \
        ideas, we'd genuinely love to talk about them or take a PR. The discussions on \
        GitHub are open:";
    let github_text_owned = github_text.to_string();
    let github_url = GITHUB_DISCUSSIONS_URL;
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "The bundle-size trade-off (honestly)".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "The place where pure-JS frameworks are clearly ahead today is \
                 bundle size. A Vue / Svelte / Angular app can ship a few tens of KB of \
                 framework code; an Idealyst web build ships a WASM binary that's \
                 typically several hundred KB compressed, even with aggressive code \
                 splitting and SSR. This is a real cost and we're not going to dress it \
                 up — it shows up on cold loads, on data-plan-constrained networks, and \
                 on first paint.".to_string(),
            )
            Typography(
                content = "The framework already does what it can on this front: \
                 wasm-split for code-splitting, SSR for first paint, runtime font \
                 fetching instead of embedding, lazy chunks for heavy subtrees. We're \
                 actively working on shrinking it further, but it's worth being upfront \
                 — if a couple hundred KB of WASM on cold load is a deal-breaker, \
                 picking a JS framework today is the right call.".to_string(),
            )
            Typography(content = github_text_owned)
            link(external = github_url) {
                Typography(content = github_url.to_string())
            }
        }
    }
}

fn footer_links() -> Element {
    ui! {
        Stack(gap = StackGap::Sm) {
            link(route = &PERFORMANCE_ROUTE, params = ()) {
                Typography(content = "See the performance page \u{2192}".to_string())
            }
            link(route = &COMPARE_WHEN_NOT_ROUTE, params = ()) {
                Typography(content = "When not to use Idealyst \u{2192}".to_string())
            }
            link(route = &COMPARISONS_ROUTE, params = ()) {
                Typography(content = "Back to all comparisons \u{2192}".to_string())
            }
        }
    }
}
