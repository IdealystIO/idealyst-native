//! Per-element visual building blocks of the welcome scene. Each
//! submodule owns the stylesheet(s) for one layer plus any
//! constants that are purely about its appearance.
//!
//! The act-by-act animation (timeline, refs, AVs, raf-driven pulse)
//! lives in [`crate::app`]; this module is intentionally inert —
//! the only thing it produces is `Rc<StyleSheet>` values.

pub mod content_layer;
pub mod dark_layer;
pub mod page;
pub mod planet;
pub mod subtitle;
pub mod sun_glare;
pub mod vignette;
pub mod welcome_phrase;
