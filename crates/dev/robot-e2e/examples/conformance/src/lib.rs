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
//! ## Dual-core (idea-lite migration)
//!
//! The crate is same-source dual-core (the newcore-app feature pattern):
//! `old-core` (default) is the walker app below, byte-for-byte what it
//! always was; `new-core` builds the same screens/suite against the
//! vocabulary + scene registry (`screens_newcore.rs`), booted on web via
//! `backend_web::newcore::start`:
//!
//! ```text
//! wasm-pack build --target web --dev crates/dev/robot-e2e/examples/conformance \
//!     -- --no-default-features --features new-core,robot
//! ```
//!
//! Old-core-ONLY surfaces are gated out of the new-core build and their
//! suites reported as blocked (never weakened): idea-ui components
//! (idea-ui's own `ui!` sites can't lower to the new core yet — SDK
//! retarget, P6) and `#[method]` component invocation (P5).

#[cfg(all(feature = "new-core", feature = "old-core"))]
compile_error!(
    "conformance: enable exactly one of `new-core` / `old-core` — one core per build \
     (the macro lowering is a build-graph-wide switch; see runtime-macros/new-core)."
);

use runtime_core::Route;

#[cfg(feature = "old-core")]
mod screens;
#[cfg(feature = "new-core")]
mod screens_newcore;
#[cfg(feature = "robot")]
mod suites;

// ---------------------------------------------------------------------------
// Per-target registration hook. Navigators + externals self-register at
// backend construction via `inventory::submit!`, so the app body is empty;
// the CLI bootstrap still calls it for app-local externals.
// ---------------------------------------------------------------------------

pub fn register_extensions<B: runtime_core::Backend>(_backend: &mut B) {}

#[cfg(feature = "sidecar")]
pub fn register_extensions_recorder(backend: &mut dev_server::WireRecordingBackend) {
    stack_navigator::recording::register(backend);
}

// ---------------------------------------------------------------------------
// Routes. The root is the primitives torture screen (always mounted at the
// bottom of the stack); `DETAIL` is a pushed screen used to exercise
// stack push/pop.
// ---------------------------------------------------------------------------

pub(crate) const ROOT: Route<()> = Route::<()>::new("root", "/");
pub(crate) const DETAIL: Route<()> = Route::<()>::new("detail", "/detail");
#[cfg(feature = "old-core")]
pub(crate) const COMPONENTS: Route<()> = Route::<()>::new("components", "/components");

/// ~1s gives the first layout/paint time to settle before the suite runs.
#[cfg(feature = "robot")]
const INITIAL_RUN_DELAY_MS: i32 = 1000;

// ===========================================================================
// OLD-core app (default) — unchanged.
// ===========================================================================

#[cfg(feature = "old-core")]
mod old_app {
    use super::*;
    use idea_ui::{install_idea_theme, light_theme};
    use runtime_core::{component, signal, Element, Ref, Screen, Signal};
    use stack_navigator::{StackBuilder, StackHandle, StackNavigator, StackScreenExt};

    /// App-wide reactive state. Lives in the root scope so it survives
    /// navigation — the suite asserts against it across push/pop.
    #[derive(Clone, Copy)]
    pub struct State {
        /// Counter incremented by a button + a pressable.
        pub count: Signal<i32>,
        /// Gates a `when` branch that reveals the slider + an extra marker.
        pub show_extra: Signal<bool>,
        /// Slider value, range 0..100.
        pub slider: Signal<f32>,
        /// Text-input contents, echoed into a greeting.
        pub name: Signal<String>,
        /// Modal open/closed.
        pub modal_open: Signal<bool>,
        /// Incremented by the modal's confirm button — proves a button NESTED
        /// inside the modal card's pressable still fires (the iOS/macOS
        /// modal-pressability regression).
        pub confirmed: Signal<i32>,
    }

    #[component]
    pub fn app() -> Element {
        install_idea_theme(light_theme());

        let state = State {
            count: signal(0),
            show_extra: signal(false),
            slider: signal(0.0_f32),
            name: signal(String::new()),
            modal_open: signal(false),
            confirmed: signal(0),
        };

        let nav: Ref<StackHandle> = Ref::new();

        // Kick off the suite once the first render has populated the registry.
        // `after_ms_detached` self-manages its handle (no cancel-on-drop).
        #[cfg(feature = "robot")]
        runtime_core::after_ms_detached(INITIAL_RUN_DELAY_MS, suites::run_all);

        let builder = StackNavigator::new(&ROOT)
            .screen(ROOT, move |_| {
                Screen::new(screens::root_page(state, nav)).title("Conformance")
            })
            .screen(DETAIL, move |_| {
                Screen::new(screens::detail_page(nav)).title("Detail")
            })
            .screen(COMPONENTS, move |_| {
                Screen::new(screens::components_page(nav)).title("Components")
            });

        runtime_core::ui! { builder.bind(nav) }
    }
}

#[cfg(feature = "old-core")]
pub use old_app::{app, State};

// ===========================================================================
// NEW-core app — the same screens/state authored against the vocabulary
// (screens_newcore.rs), on the vocabulary stack navigator. The COMPONENTS
// screen (idea-ui) and the `#[method]` counter are old-core-only; their
// suites are gated out (blocked, reported — see suites.rs).
// ===========================================================================

#[cfg(feature = "new-core")]
pub use screens_newcore::{app_newcore, State};

// New-core web boot: `wasm-pack build --target web` produces a module
// whose start fn mounts into `#app` — the newcore-web-smoke pattern.
#[cfg(all(target_arch = "wasm32", feature = "new-core"))]
mod web_entry {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(start)]
    pub fn boot() {
        // Console logger so `[e2e]` / `[E2E-RESULT]` lines reach the
        // devtools console (the CLI wrapper normally installs this).
        backend_web::install_logger();
        backend_web::newcore::start(crate::app_newcore);
    }
}
