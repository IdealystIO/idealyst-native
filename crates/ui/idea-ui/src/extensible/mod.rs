//! Components built on the open-trait modifier system from
//! [`idea_theme::extensible`]. Each component takes its modifier props
//! as `Rc<dyn Trait>` (Tone, Variant, ButtonSize, Shape, …) — apps
//! pass either built-in ZSTs from `idea_theme::tone::*` etc. or their
//! own marker types implementing the same traits.
//!
//! Sits alongside the original closed-enum-based components in
//! [`crate::components`] — both ship for now. The extensible variants
//! are the direction the library is moving.

pub mod button;
pub mod typography;
