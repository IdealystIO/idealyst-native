//! The starting code of every [`Fiddle`](crate::fiddle::Fiddle).
//!
//! Each file is a complete `app.rs` — what a reader edits and what the
//! fiddle server builds — and is also compiled here, as a module of this
//! crate. So `cargo check -p tutorial` proves every starter builds against
//! the framework as it is now, before a reader ever presses Run. A lesson
//! hands one to its fiddle with `include_str!`, so the text a reader sees
//! is byte-for-byte what was checked.
//!
//! Rules for a starter: it defines `pub fn app() -> Element`; it uses only
//! what the fiddle template depends on (`runtime_core`, `idea_ui`); its
//! comments are written to the reader; and it carries no `#[allow]` (those
//! live on the `mod` lines in each track's own `mod.rs`, where `app` is
//! otherwise unused).

pub mod building_ui;
pub mod component_styles;
pub mod getting_started;
pub mod styling;
