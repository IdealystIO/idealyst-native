//! `baseline` — the most trivial possible idealyst app: a single text node.
//!
//! Exists purely to measure the framework's *floor* web bundle size — the
//! irreducible cost of `runtime-core` + `backend-web` + the walker, with no
//! `idea-ui`, no SDKs, no navigator, no animation. Everything above this in a
//! real app is additive; this is the baseline to compare against.

use runtime_core::{ui, Element};

/// The whole app: one line of text.
pub fn app() -> Element {
    ui! {
        view {
            text { "Hello, Idealyst" }
        }
    }
}

/// SDK-handler registration hook the CLI-generated wrappers invoke before
/// mount. This app registers nothing, so it's an empty generic over `Backend`.
pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}
