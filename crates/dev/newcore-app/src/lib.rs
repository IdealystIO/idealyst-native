//! newcore-app — the macro/handler integration app (see Cargo.toml).
//!
//! `src/app.rs` is the AUTHORED source: `ui!` + `#[component]` only.
//! This file is the crate's prelude — the one place the shared authoring
//! surface (`Element`, `signal`, `memo`, `Signal`, the primitive value
//! types) is named, so `app.rs` reads as ordinary author code with a
//! single `use crate::prelude::*;`.
//!
//! The prelude exists because this crate used to be the DUAL-CORE proof:
//! the same `app.rs` compiled against the old walker and against runtime
//! v2, and the prelude was the only per-core wiring. With one core it is
//! simply the crate's import surface.

/// The authoring surface `app.rs` builds against.
pub mod prelude {
    pub use runtime_vocabulary::glue::{
        memo, signal, Color, Easing, Element, Memo, Ref, Signal, Tokenized, ViewHandle,
    };
    // The robot watch surface: this crate enables
    // `runtime-vocabulary/robot`, so the app's `watch_signal` calls
    // register live entries the bridge's `read_signal` /
    // `list_watched_signals` verbs (and robot-test's `assert_signal`)
    // read.
    pub use runtime_vocabulary::robot::watch_signal;
    // Primitive value types (`overlay` placement/backdrop,
    // `anchored_overlay` anchor/side, `presence` anims, `flat_list`
    // sizing), reached through the glue's `primitives::…` mirror of the
    // shared substrate.
    pub use runtime_vocabulary::glue::primitives::flat_list::fixed_size;
    pub use runtime_vocabulary::glue::primitives::overlay::{
        AnchorTarget, BackdropMode, ElementSide, ViewportPlacement,
    };
    pub use runtime_vocabulary::glue::primitives::presence::PresenceAnim;
    // `ui!` emits `::runtime_vocabulary::glue::…` paths; the dispatch
    // traits and coercions are resolved absolutely by the macro, so
    // nothing else needs importing here. `Tokenized`/`Color` serve the
    // `stylesheet!` bodies.
    pub use std::rc::Rc;
}

pub mod app;
