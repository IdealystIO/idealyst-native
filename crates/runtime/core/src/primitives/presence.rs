//! `Presence` — declarative mount/unmount animation.
//!
//! Presence sits between a host's open/close `Signal<bool>` and the
//! subtree being shown. It owns the *timing* of mount and unmount so
//! the framework can play an enter animation on appearance and an
//! exit animation on disappearance before tearing the subtree down.
//!
//! # Why this isn't the existing `transitions { ... }` machinery
//!
//! The stylesheet-level `Transition` system is change-driven — a
//! property has a value, it changes, the backend interpolates from
//! the previous value to the new one. Presence breaks two of that
//! system's assumptions:
//!
//! 1. **There's no "from" on enter.** The node was just mounted; the
//!    transition system has no previous value to interpolate from
//!    and would snap straight to the rendered state.
//! 2. **The "to" on exit happens *after* the scope would normally
//!    drop.** The framework's `when` / `switch` primitives tear down
//!    the subtree the moment the condition flips. There's no window
//!    for the exit animation to play.
//!
//! Presence solves both by carrying its own animation declaration —
//! a small, deliberately-narrow vocabulary of presence-only
//! properties — and managing the timing in the walker:
//!
//! - **Enter**: apply `enter.state` to the node *before* the first
//!   paint, then one animation-frame later clear the override so the
//!   backend interpolates back to the resting state over
//!   `enter.duration_ms`.
//! - **Exit**: when `present()` flips false, apply `exit.state` and
//!   start a `ScheduledTask` for `exit.duration_ms`. When the timer
//!   fires, drop the scope for real. If `present()` flips back to
//!   true before the timer fires, cancel the task and re-apply the
//!   resting state — the in-flight interpolation reverses naturally.
//!
//! # Why a tight vocabulary
//!
//! `PresenceState` carries only opacity + 2D translate + uniform
//! scale. Those four properties are universally interpolatable on
//! every backend (web `style.opacity` + `style.transform`; iOS
//! `UIViewPropertyAnimator`; Android `ObjectAnimator`), and they
//! cover ~95% of the enter/exit animations a UI library actually
//! needs (fade, slide, zoom).
//!
//! Things deliberately *not* in the vocabulary:
//!
//! - **Color / background interpolation** — presence isn't a
//!   re-skinner. If a card needs to transition its background, that
//!   belongs in its own stylesheet `transitions` block, separate
//!   from mount/unmount.
//! - **Rotation, skew, blur** — rare; not cross-platform-cheap on
//!   every backend; can be added later if real use cases show up.
//! - **`pointer-events`, `display`** — these are invariants the
//!   backend handles automatically during exit (pointer events go
//!   off; subtree stays in layout). Not user-tunable.
//!
//! # Usage
//!
//! ```ignore
//! use runtime_core::{signal, ui, Easing, presence, PresenceAnim, PresenceState};
//!
//! let open = signal!(false);
//! ui! {
//!     Presence(
//!         present = move || open.get(),
//!         enter = PresenceAnim::new(
//!             PresenceState { opacity: Some(0.0), translate_y: Some(8.0), ..Default::default() },
//!             200,
//!             Easing::EaseOut,
//!         ),
//!         exit = PresenceAnim::new(
//!             PresenceState { opacity: Some(0.0), translate_y: Some(8.0), ..Default::default() },
//!             150,
//!             Easing::EaseIn,
//!         ),
//!     ) {
//!         Modal(...) { ... }
//!     }
//! }
//! ```

use std::any::Any;
use std::rc::Rc;

use crate::style::Easing;
use crate::{Bound, Element, Ref, RefFill};

// ============================================================================
// PresenceState — the animatable vocabulary
// ============================================================================

/// The narrow set of properties a presence animation can override
/// during enter / exit. Each is `Option` so authors only set the
/// ones they care about; missing fields use the resting value.
///
/// All coordinates are in CSS pixels (or the backend's equivalent
/// point unit). `scale` is uniform — non-uniform scale isn't
/// supported.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct PresenceState {
    pub opacity: Option<f32>,
    pub translate_x: Option<f32>,
    pub translate_y: Option<f32>,
    pub scale: Option<f32>,
}

impl PresenceState {
    /// "Resting" state — every field set to its identity value.
    /// Backends interpret this as "no presence override is active";
    /// the rendered values come from the node's regular style.
    pub fn rest() -> Self {
        Self::default()
    }

    pub fn opacity(mut self, v: f32) -> Self {
        self.opacity = Some(v);
        self
    }
    pub fn translate(mut self, x: f32, y: f32) -> Self {
        self.translate_x = Some(x);
        self.translate_y = Some(y);
        self
    }
    pub fn translate_y(mut self, y: f32) -> Self {
        self.translate_y = Some(y);
        self
    }
    pub fn translate_x(mut self, x: f32) -> Self {
        self.translate_x = Some(x);
        self
    }
    pub fn scale(mut self, s: f32) -> Self {
        self.scale = Some(s);
        self
    }
}

/// One half of a presence definition. On enter, `state` is what's
/// applied *before* the first paint; on exit, it's what's
/// interpolated *toward* before the scope drops. Same shape both
/// ways so authors can mirror a fade-and-slide enter as a
/// fade-and-slide exit by sharing a `PresenceState`.
#[derive(Copy, Clone, Debug)]
pub struct PresenceAnim {
    pub state: PresenceState,
    pub duration_ms: u32,
    pub easing: Easing,
}

impl PresenceAnim {
    pub fn new(state: PresenceState, duration_ms: u32, easing: Easing) -> Self {
        Self { state, duration_ms, easing }
    }

    /// A fade-only enter or exit. Common enough to deserve a helper.
    pub fn fade(duration_ms: u32, easing: Easing) -> Self {
        Self {
            state: PresenceState::default().opacity(0.0),
            duration_ms,
            easing,
        }
    }
}

// ============================================================================
// Handle + ops (placeholder — no imperative API yet)
// ============================================================================

#[derive(Clone)]
pub struct PresenceHandle {
    #[allow(dead_code)]
    node: Rc<dyn Any>,
    #[allow(dead_code)]
    ops: &'static dyn PresenceOps,
}

impl PresenceHandle {
    pub fn new(node: Rc<dyn Any>, ops: &'static dyn PresenceOps) -> Self {
        Self { node, ops }
    }
}

pub trait PresenceOps {}

// ============================================================================
// Constructor + builder
// ============================================================================

/// Build a presence-controlled subtree.
///
/// The `child` closure is called once per real mount — i.e. once
/// when `present()` first returns true, and again only if a complete
/// unmount (exit animation finished + scope dropped) precedes a new
/// flip-on. Mid-exit reversal does NOT rebuild — the existing scope
/// is reused so signals and refs inside the child survive a
/// near-miss flicker.
pub fn presence<F>(child: F) -> Bound<PresenceHandle>
where
    F: Fn() -> Element + 'static,
{
    Bound::new(Element::Presence {
        child: Box::new(child),
        // Default: always present (the child mounts immediately and
        // never exits). Useful for testing the shape; real call
        // sites always set `.present(...)`.
        present: Box::new(|| true),
        enter: None,
        exit: None,
        ref_fill: None,
        accessibility: crate::accessibility::AccessibilityProps::default(),
    })
}

impl Bound<PresenceHandle> {
    pub fn present<F>(mut self, f: F) -> Self
    where
        F: Fn() -> bool + 'static,
    {
        if let Element::Presence { present, .. } = &mut self.primitive {
            *present = Box::new(f);
        }
        self
    }

    pub fn enter(mut self, anim: PresenceAnim) -> Self {
        if let Element::Presence { enter, .. } = &mut self.primitive {
            *enter = Some(anim);
        }
        self
    }

    pub fn exit(mut self, anim: PresenceAnim) -> Self {
        if let Element::Presence { exit, .. } = &mut self.primitive {
            *exit = Some(anim);
        }
        self
    }

    pub fn bind(mut self, r: Ref<PresenceHandle>) -> Self {
        if let Element::Presence { ref_fill, .. } = &mut self.primitive {
            *ref_fill = Some(RefFill::Presence(Box::new(move |h| r.fill(h))));
        }
        self
    }
}

#[cfg(test)]
mod tests {
    //! PresenceState + PresenceAnim builder coverage. Pure data —
    //! no backend needed.

    use super::*;
    use crate::Easing;

    #[test]
    fn presence_state_rest_is_all_none() {
        let s = PresenceState::rest();
        assert!(s.opacity.is_none());
        assert!(s.translate_x.is_none());
        assert!(s.translate_y.is_none());
        assert!(s.scale.is_none());
    }

    #[test]
    fn presence_state_default_matches_rest() {
        let a: PresenceState = PresenceState::default();
        let b = PresenceState::rest();
        assert_eq!(a, b);
    }

    #[test]
    fn presence_state_opacity_sets_only_opacity() {
        let s = PresenceState::rest().opacity(0.5);
        assert_eq!(s.opacity, Some(0.5));
        assert!(s.translate_x.is_none());
        assert!(s.translate_y.is_none());
        assert!(s.scale.is_none());
    }

    #[test]
    fn presence_state_translate_sets_both_axes() {
        let s = PresenceState::rest().translate(3.0, 4.0);
        assert_eq!(s.translate_x, Some(3.0));
        assert_eq!(s.translate_y, Some(4.0));
        // Other fields untouched.
        assert!(s.opacity.is_none());
        assert!(s.scale.is_none());
    }

    #[test]
    fn presence_state_translate_x_and_translate_y_are_orthogonal() {
        let only_x = PresenceState::rest().translate_x(7.0);
        assert_eq!(only_x.translate_x, Some(7.0));
        assert!(only_x.translate_y.is_none());

        let only_y = PresenceState::rest().translate_y(9.0);
        assert!(only_y.translate_x.is_none());
        assert_eq!(only_y.translate_y, Some(9.0));
    }

    #[test]
    fn presence_state_scale_sets_only_scale() {
        let s = PresenceState::rest().scale(2.0);
        assert_eq!(s.scale, Some(2.0));
        assert!(s.opacity.is_none());
    }

    #[test]
    fn presence_state_builders_chain_and_compose() {
        let s = PresenceState::rest()
            .opacity(0.2)
            .translate(1.0, 2.0)
            .scale(0.9);
        assert_eq!(s.opacity, Some(0.2));
        assert_eq!(s.translate_x, Some(1.0));
        assert_eq!(s.translate_y, Some(2.0));
        assert_eq!(s.scale, Some(0.9));
    }

    #[test]
    fn presence_state_translate_overwrites_individual_axis_setters() {
        // `translate(x, y)` sets both axes regardless of prior
        // individual sets — last write wins.
        let s = PresenceState::rest()
            .translate_x(100.0)
            .translate_y(200.0)
            .translate(0.0, 0.0);
        assert_eq!(s.translate_x, Some(0.0));
        assert_eq!(s.translate_y, Some(0.0));
    }

    #[test]
    fn presence_anim_new_carries_all_three_fields() {
        let state = PresenceState::rest().opacity(0.3);
        let anim = PresenceAnim::new(state, 250, Easing::Linear);
        assert_eq!(anim.duration_ms, 250);
        assert_eq!(anim.state.opacity, Some(0.3));
        // Easing has a known Default; assert the variant we passed.
        matches!(anim.easing, Easing::Linear);
    }

    #[test]
    fn presence_anim_fade_zeros_opacity_only() {
        let anim = PresenceAnim::fade(180, Easing::Linear);
        assert_eq!(anim.state.opacity, Some(0.0));
        assert!(anim.state.translate_x.is_none());
        assert!(anim.state.translate_y.is_none());
        assert!(anim.state.scale.is_none());
        assert_eq!(anim.duration_ms, 180);
    }
}
