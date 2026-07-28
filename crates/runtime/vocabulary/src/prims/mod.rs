//! Payload types for the built-in primitives (P2b) — one struct per
//! primitive, carried inside an [`Element::Item`](runtime_scene::Element)
//! behind a [`PrimCell`].
//!
//! # Prop model
//!
//! - [`runtime_world::Value<T>`] for reactive-capable props — the caller
//!   picks reactivity per prop (`Const` from a literal, `Dyn` from a
//!   signal/closure) and the handler binds it (zero effects for `Const`,
//!   exactly one for `Dyn`).
//! - `Rc<dyn Fn(...)>` for callbacks.
//! - Plain fields for inert config (`AccessibilityProps`, `SafeAreaSides`,
//!   icon data, …), reusing runtime-core's data types (the sanctioned
//!   transitional dependency — those types migrate here at P7).
//! - [`StyleProp`](crate::style_attach::StyleProp) for the style slot
//!   (static resolved rules / dynamic rules closure — the two P2 paths).
//!
//! Field shapes are derived from what the old walker passes to
//! `create_*`/`update_*` (each payload's docs name its walker module), so
//! the generic handlers can reproduce the walker's backend-call streams —
//! the property `scene-parity`'s full-op goldens pin.

use std::cell::RefCell;

mod media;
mod text;
mod view;
mod widgets;

pub use media::{IconPrim, ImagePrim, LinkPrim};
pub use text::{ButtonPrim, TextPrim, TextSourceProp};
pub use view::{PressablePrim, ScrollViewPrim, ViewPrim};
pub use widgets::{ActivityIndicatorPrim, SliderPrim, TextAreaPrim, TextInputPrim, TogglePrim};

/// Single-mount wrapper for primitive payloads.
///
/// The scene registry hands handlers a shared `&Rc<T>` payload, but a
/// payload's props (a `Value<T>`, a `Box<dyn Fn>` , a `FnOnce` ref-fill)
/// must be consumed **by move** at mount. An `Element` is single-use
/// (realize consumes it), so every payload mounts at most once —
/// `PrimCell` makes that ownership transfer explicit: builders wrap the
/// payload at construction, handlers [`take`](PrimCell::take) it at
/// mount, and a second take (impossible through the scene; reachable only
/// by a hand-rolled handler bug) panics with a diagnostic.
pub struct PrimCell<T>(RefCell<Option<T>>);

impl<T> PrimCell<T> {
    pub fn new(payload: T) -> Self {
        PrimCell(RefCell::new(Some(payload)))
    }

    /// Move the payload out for mounting. Panics if already taken.
    pub fn take(&self) -> T {
        self.0.borrow_mut().take().unwrap_or_else(|| {
            panic!(
                "runtime-vocabulary: primitive payload `{}` mounted twice — an Element is \
                 single-use; a handler must take() its payload exactly once",
                std::any::type_name::<T>()
            )
        })
    }
}
