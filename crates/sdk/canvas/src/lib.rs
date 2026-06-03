//! `canvas` — the author-facing facade for the 2D-drawing SDK.
//!
//! Screens import this crate for the small, consistent `canvas::`
//! namespace; it re-exports the renderer-agnostic abstraction from
//! [`canvas_core`]. Pick a renderer at app bootstrap by calling
//! `canvas_native::register(&mut backend)` **or**
//! `canvas_vello::register(&mut backend)` (exactly one — the
//! `Element::External` registry is `TypeId`-keyed, last write wins).
//!
//! See [`canvas_core`] for the full API and usage example.
#![deny(missing_docs)]

pub use canvas_core::*;
