//! newcore-app — the P3a integration example (see Cargo.toml).
//!
//! `src/app.rs` is the AUTHORED source: `ui!` + `#[component]` only,
//! written once, compiled against either core. This file is the ONLY
//! per-core wiring — a prelude module that resolves the shared surface
//! (`Element`, `signal`, `memo`, `Signal`) to the selected core. That is
//! the app-crate contract of the `new-core` opt-in: the tree, the
//! components, the handlers, the control flow do not change; the crate
//! root picks the core.

#[cfg(all(feature = "new-core", feature = "old-core"))]
compile_error!(
    "newcore-app: enable exactly one of `new-core` / `old-core` — one core per build \
     (the macro lowering is a build-graph-wide switch; see runtime-macros/new-core)."
);

/// The shared authoring surface, resolved per core.
#[cfg(feature = "new-core")]
pub mod prelude {
    pub use runtime_vocabulary::glue::{
        memo, signal, Color, Easing, Element, Memo, Ref, Signal, Tokenized, ViewHandle,
    };
    // The robot watch surface (P5 remainder): this build enables
    // `runtime-vocabulary/robot`, so the app's `watch_signal` calls
    // register live entries the bridge's `read_signal` /
    // `list_watched_signals` verbs (and robot-test's `assert_signal`)
    // read.
    pub use runtime_vocabulary::robot::watch_signal;
    // The P3-set primitive value types (`overlay` placement/backdrop,
    // `anchored_overlay` anchor/side, `presence` anims, `flat_list`
    // sizing) — the SAME runtime-core data types on both cores, reached
    // through the glue's mirrored `primitives::…` paths here.
    pub use runtime_vocabulary::glue::primitives::flat_list::fixed_size;
    pub use runtime_vocabulary::glue::primitives::overlay::{
        AnchorTarget, BackdropMode, ElementSide, ViewportPlacement,
    };
    pub use runtime_vocabulary::glue::primitives::presence::PresenceAnim;
    // `ui!` emits `::runtime_vocabulary::glue::…` paths under `new-core`;
    // the dispatch traits and coercions are resolved absolutely by the
    // macro, so nothing else needs importing here. `Tokenized`/`Color`
    // serve the `stylesheet!` bodies (the sheet DATA MODEL is shared
    // with the old core — these are re-exports of the same types).
    pub use std::rc::Rc;
}

#[cfg(all(feature = "old-core", not(feature = "new-core")))]
pub mod prelude {
    pub use runtime_core::primitives::flat_list::fixed_size;
    pub use runtime_core::primitives::overlay::BackdropMode;
    pub use runtime_core::primitives::portal::{AnchorTarget, ElementSide, ViewportPlacement};
    pub use runtime_core::primitives::presence::PresenceAnim;
    pub use runtime_core::{
        memo, signal, Color, Easing, Element, Ref, Signal, Tokenized, ViewHandle,
    };
    pub use std::rc::Rc;

    /// Same-source shim for the robot watch surface. This old-core
    /// build is the CHECK-ONLY leg (it never enables
    /// `runtime-core/robot`, whose real `watch_signal` lives behind
    /// that gate), so the app's calls compile to nothing here — the
    /// new-core prelude routes to the live vocabulary registry.
    pub fn watch_signal<S>(_name: impl Into<String>, _signal: S) {}
}

#[cfg(any(feature = "new-core", feature = "old-core"))]
pub mod app;
