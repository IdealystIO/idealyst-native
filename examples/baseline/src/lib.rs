//! `baseline` — the most trivial possible idealyst app: a single text node.
//!
//! Exists purely to measure the framework's *floor* web bundle size — the
//! irreducible cost of the runtime (`runtime-world` + `runtime-scene` +
//! `runtime-vocabulary`, reached through the facade alias) + `backend-web`,
//! with no `idea-ui`, no SDKs, no navigator, no animation. Everything above
//! this in a real app is additive; this is the baseline to compare against.

use runtime_core::{ui, Element};

/// The whole app: one line of text.
pub fn app() -> Element {
    ui! {
        view {
            text { "Hello, Idealyst" }
        }
    }
}

/// SDK-handler registration seam the CLI-generated wrappers invoke after
/// `runtime_vocabulary::register_builtins`. This app registers nothing, so it
/// is an empty generic over the scene registry's host.
pub fn register_scene_extensions<H: runtime_scene::Host>(
    _registry: &mut runtime_scene::Registry<H>,
) {
}
