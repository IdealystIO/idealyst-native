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
    pub use runtime_vocabulary::glue::{memo, signal, Color, Element, Memo, Signal, Tokenized};
    // `ui!` emits `::runtime_vocabulary::glue::…` paths under `new-core`;
    // the dispatch traits and coercions are resolved absolutely by the
    // macro, so nothing else needs importing here. `Tokenized`/`Color`
    // serve the `stylesheet!` bodies (the sheet DATA MODEL is shared
    // with the old core — these are re-exports of the same types).
    pub use std::rc::Rc;
}

#[cfg(all(feature = "old-core", not(feature = "new-core")))]
pub mod prelude {
    pub use runtime_core::{memo, signal, Color, Element, Signal, Tokenized};
    pub use std::rc::Rc;
}

#[cfg(any(feature = "new-core", feature = "old-core"))]
pub mod app;
