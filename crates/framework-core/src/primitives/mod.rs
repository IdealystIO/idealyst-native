//! Per-primitive modules. Each primitive (Image, TextInput, Toggle,
//! ScrollView, ...) gets its own file containing its handle type,
//! ops trait, constructor function, and any `Bound<H>`-specific
//! builder methods.
//!
//! The `Primitive` enum itself stays in the crate root — Rust's
//! enum-variant constraint means we can't split it across files
//! without sum-type machinery (`Box<dyn Primitive>`-style) and the
//! cost of that doesn't pay off at this scale. So this module is
//! about *per-primitive surface* (handles, builders, constructors),
//! not the enum data.

pub mod activity_indicator;
pub mod flat_list;
pub mod graphics;
pub mod icon;
pub mod image;
pub mod link;
pub mod navigator;
pub mod overlay;
pub mod presence;
pub mod scroll_view;
pub mod slider;
pub mod text_input;
pub mod toggle;
pub mod video;
pub mod virtualizer;
pub mod web_view;
