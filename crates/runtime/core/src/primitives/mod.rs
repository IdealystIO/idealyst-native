//! Per-primitive modules. Each primitive (Image, TextInput, Toggle,
//! ScrollView, ...) gets its own file containing its handle type,
//! ops trait, constructor function, and any `Bound<H>`-specific
//! builder methods.
//!
//! The `Element` enum itself stays in the crate root — Rust's
//! enum-variant constraint means we can't split it across files
//! without sum-type machinery (`Box<dyn Element>`-style) and the
//! cost of that doesn't pay off at this scale. So this module is
//! about *per-primitive surface* (handles, builders, constructors),
//! not the enum data.

pub mod activity_indicator;
// Authoring-only wrapper around `Element::Virtualizer` — gated with the
// primitive so a disabled `prim-virtualizer` turns any use of `flat_list`
// into a compile error naming the missing fn (the model types in
// `virtualizer` stay ungated: `Element` and the wire need them).
#[cfg(feature = "prim-virtualizer")]
pub mod flat_list;
pub mod graphics;
pub mod icon;
pub mod image;
// `key` moved wholesale to runtime-shared (pure data types).
pub use runtime_shared::primitives::key;
pub mod lazy;
pub mod link;
pub mod navigator;
pub mod overlay;
pub mod portal;
pub mod presence;
pub mod scroll_view;
pub mod slider;
pub mod text_area;
pub mod text_input;
pub mod toggle;
pub mod virtualizer;
