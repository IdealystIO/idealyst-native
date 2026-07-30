//! `canvas` — the author-facing facade for the 2D-drawing SDK.
//!
//! Screens import this crate for the small, consistent `canvas::`
//! namespace; it re-exports the renderer-agnostic abstraction from
//! [`canvas_core`]. Pick a renderer at app bootstrap by passing exactly
//! one of `canvas_native::register` / `canvas_vello::register` to the
//! boot entry's registry seam (the scene registry is `TypeId`-keyed, so
//! a second `register` for the same payload just wins).
//!
//! See [`canvas_core`] for the full API and usage example.
#![deny(missing_docs)]

pub use canvas_core::*;
