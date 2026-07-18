//! "When not to use Idealyst" — the honest caveats page. WASM still
//! young in places, Rust learning curve, bundle size on the web. Linked
//! from every other comparison so the reader meets the trade-offs head-
//! on rather than discovering them mid-build.

use runtime_core::{ui, Element};
use idea_ui::{typography_kind, Stack, StackGap, Typography};

use crate::pages::common::PageHeader;
use crate::routes::{COMPARISONS_ROUTE, WHY_RUST_ROUTE};
use crate::shell::layout;

const GITHUB_DISCUSSIONS_URL: &str =
    "https://github.com/IdealystIO/idealyst-native/discussions";

pub fn page() -> Element {
    let content = ui! {
        Stack(gap = StackGap::Xl) {
            PageHeader(
                title = "When not to use Idealyst",
                blurb = "Idealyst is a new frontier in app development — rich in \
                 potential, with the chance of choppy waters ahead. The framework is \
                 worth your time if that trade-off sounds like fun. If you're shipping \
                 something on a deadline where every edge case has to be already-solved, \
                 the rest of this page is for you.",
            )
            wasm_maturity()
            rust_curve()
            bundle_size()
            closing()
            footer_links()
        }
    };
    layout(content)
}

fn wasm_maturity() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "WASM is still maturing on the edges".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Browser support for WebAssembly is universal and has been for \
                 years, but the toolchain around it — wasm-bindgen, wasm-split, the \
                 streaming-compile path, source-map fidelity in production builds — is \
                 still moving. Idealyst hits the edges of that toolchain more than a \
                 typical web app does. Most of the time it works flawlessly; when it \
                 doesn't, the failure mode can be obscure (we have a memory in this \
                 codebase about a stack-overflow that looked like a wasm-bindgen \
                 externref bug for weeks before the real cause turned up).".to_string(),
            )
            Typography(
                content = "If your team doesn't have appetite for occasionally getting \
                 your hands dirty in a wasm trap or a compiler regression, that's a \
                 reasonable reason to wait.".to_string(),
                muted = true,
            )
        }
    }
}

fn rust_curve() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Rust is a learning curve".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "Many web developers haven't written Rust before, and the \
                 ownership / lifetime model is a real intellectual investment to pick up. \
                 The framework has been designed to keep authoring-surface ergonomics \
                 close to React's mental model — components, props, signals, JSX-ish \
                 syntax — but the language underneath is still Rust, and it will hold \
                 you to its rules.".to_string(),
            )
            Typography(
                content = "If most of your team is more comfortable in TypeScript than in \
                 Rust, factor in the ramp time. (The flip side: once they're past it, \
                 most of the runtime classes of bug that web frameworks ship with stop \
                 existing.)".to_string(),
            )
        }
    }
}

fn bundle_size() -> Element {
    let github_intro = "WASM bundle size is one of the most active areas of focus right \
        now, and ideas / PRs from the community are welcome — if you have a take on how \
        to shrink it further, the GitHub discussions are open:";
    let intro_owned = github_intro.to_string();
    let github_url = GITHUB_DISCUSSIONS_URL;
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "WASM bundle size".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "An Idealyst web build ships a WASM binary that's heavier than \
                 a hand-tuned pure-JS bundle. Even with aggressive code splitting and \
                 SSR, the framework's runtime is in the few-hundred-KB range compressed. \
                 If your traffic comes in on cold loads over constrained networks and \
                 every kilobyte matters, this is a real cost to weigh.".to_string(),
            )
            Typography(content = intro_owned)
            link(external = github_url) {
                Typography(content = github_url.to_string())
            }
        }
    }
}

fn closing() -> Element {
    ui! {
        Stack(gap = StackGap::Md) {
            Typography(
                content = "Treat it like new frontier territory".to_string(),
                kind = typography_kind::H2,
            )
            Typography(
                content = "None of this is meant to talk you out of trying it. The \
                 framework already ships real apps to real platforms, and the team \
                 behind it (and the issue tracker behind it) is responsive. But the \
                 promise of \"truly cross-platform native\" is a frontier promise, not \
                 a settled one. Go in with that framing and the rough edges become \
                 interesting instead of frustrating.".to_string(),
            )
        }
    }
}

fn footer_links() -> Element {
    ui! {
        Stack(gap = StackGap::Sm) {
            link(route = &WHY_RUST_ROUTE, params = ()) {
                Typography(content = "Why Rust \u{2192}".to_string())
            }
            link(route = &COMPARISONS_ROUTE, params = ()) {
                Typography(content = "Back to all comparisons \u{2192}".to_string())
            }
        }
    }
}
