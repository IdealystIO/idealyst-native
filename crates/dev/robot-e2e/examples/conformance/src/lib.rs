//! `conformance` — a cross-platform **conformance app** that mounts the
//! framework's primitives, idea-ui components, and navigators in
//! deliberately *weird* configurations (reactive labels, conditional
//! mount/unmount, a portal modal whose card wraps interactive content,
//! nested scroll), each tagged with a stable `test_id`, and drives them
//! with an in-app [`robot_e2e`] suite.
//!
//! The point is regression confidence: run the SAME suite on every backend
//! and get one machine-readable verdict per platform.
//!
//! ## Run it
//!
//! ```text
//! idealyst dev --macos --local    # or --ios / --android / --web / --terminal
//! ```
//!
//! The suite auto-runs ~1s after launch. Each step logs an `[e2e]` line and
//! the run ends with a single `[E2E-RESULT] {…}` line for the orchestrator
//! to scrape:
//!
//! - **macOS / terminal**: stderr in the launching shell.
//! - **iOS**: `xcrun simctl spawn booted log show | grep E2E`.
//! - **Android**: `adb logcat | grep E2E`.
//! - **web**: the browser devtools console.
//!
//! ## Web build
//!
//! ```text
//! wasm-pack build --target web --dev crates/dev/robot-e2e/examples/conformance
//! ```
//!
use runtime_vocabulary::glue::Route;

mod screens;
#[cfg(feature = "robot")]
mod suites;

// ---------------------------------------------------------------------------
// Per-target registration hook. The framework's own primitives (navigators
// included) are installed by `runtime_vocabulary::register_builtins`, and
// this app registers no third-party payloads — so the seam is empty. It
// exists because every generated wrapper calls it.
// ---------------------------------------------------------------------------

pub fn register_scene_extensions<H: runtime_scene::Host>(_registry: &mut runtime_scene::Registry<H>) {
}

#[cfg(feature = "sidecar")]
pub fn register_scene_extensions_recorder(
    registry: &mut runtime_scene::Registry<dev_server::WireRecordingBackend>,
) {
    register_scene_extensions(registry);
}

// ---------------------------------------------------------------------------
// Routes. The root is the primitives torture screen (always mounted at the
// bottom of the stack); `DETAIL` is a pushed screen used to exercise
// stack push/pop.
// ---------------------------------------------------------------------------

pub(crate) const ROOT: Route<()> = Route::<()>::new("root", "/");
pub(crate) const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");
pub(crate) const COMPONENTS: Route<()> = Route::<()>::new("components", "/components");

/// ~1s gives the first layout/paint time to settle before the suite runs.
#[cfg(feature = "robot")]
const INITIAL_RUN_DELAY_MS: i32 = 1000;

pub use screens::{app, State};

// Web boot: `wasm-pack build --target web` produces a module whose start
// fn mounts into `#app`.
#[cfg(target_arch = "wasm32")]
mod web_entry {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(start)]
    pub fn boot() {
        // Console logger so `[e2e]` / `[E2E-RESULT]` lines reach the
        // devtools console (the CLI wrapper normally installs this).
        backend_web::install_logger();
        backend_web::newcore::start(crate::app);
    }
}
