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

use idea_ui::{install_idea_theme, light_theme};
use runtime_core::{component, signal, Element, Ref, Route, Screen, Signal};
use stack_navigator::{Navigator, StackBuilder, StackHandle, StackScreenExt};

mod screens;
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
        count: signal!(0),
        show_extra: signal!(false),
        slider: signal!(0.0_f32),
        name: signal!(String::new()),
        modal_open: signal!(false),
        confirmed: signal!(0),
    };

    let nav: Ref<StackHandle> = Ref::new();

    // Kick off the suite once the first render has populated the registry.
    // `after_ms_detached` self-manages its handle (no cancel-on-drop).
    #[cfg(feature = "robot")]
    runtime_core::after_ms_detached(INITIAL_RUN_DELAY_MS, suites::run_all);

    let builder = Navigator::new(&ROOT)
        .screen(ROOT, move |_| {
            Screen::new(screens::root_page(state, nav)).title("Conformance")
        })
        .screen(DETAIL, move |_| {
            Screen::new(screens::detail_page(nav)).title("Detail")
        });

    runtime_core::ui! { builder.bind(nav) }
}

/// ~1s gives the first layout/paint time to settle before the suite runs.
#[cfg(feature = "robot")]
const INITIAL_RUN_DELAY_MS: i32 = 1000;
