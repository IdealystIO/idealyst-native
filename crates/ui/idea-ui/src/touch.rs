//! Minimum touch target for interactive controls — mobile-first.
//!
//! Every tap target on a phone should be at least
//! [`MIN_TOUCH_TARGET_PX`] on each side: Apple's HIG asks for 44 pt, WCAG
//! 2.5.5 for 44 CSS px (Material's 48 dp is the same guideline, rounded
//! for its 8 dp grid). A control's natural size is set by its padding and
//! type scale, which suits a pointer but lands well under that on a phone
//! — a `size::Md` Button is ~33 pt tall.
//!
//! [`touch_target`] adds the floor to a component slot through the style
//! system's responsive overlays, so it premints (`@media` on web, the
//! breakpoint merge on native) and costs nothing at runtime:
//!
//! - the slot's **base** (the `Xs` layout — phones in portrait) gets
//!   `min_width` / `min_height` = [`MIN_TOUCH_TARGET_PX`];
//! - the [`TOUCH_TARGET_RELEASE`] breakpoint overlay sets both back to
//!   `auto`, so wider layouts keep the control's natural size.
//!
//! It is a floor, not a size: a control already taller or wider than the
//! target is unchanged at every width.
//!
//! The release is `Md`, not `Sm`: the `Sm` band (640–767) is large phones
//! in landscape and small tablets — still fingers. Width is the only
//! signal the style system can branch on; the primary pointer kind is a
//! runtime signal (`runtime_core::primary_pointer`), not an overlay axis.
//!
//! # Opting out
//!
//! - One instance: pass [`no_touch_target`] as the slot override, e.g.
//!   `styles = ButtonSlotStyles::default().root(no_touch_target())` — the
//!   override layer wins over the base at every width.
//! - App-wide, one component: restyle its base,
//!   `ButtonStyles::restyle(|s| { s.root().base(|_| no_touch_target()); })`.
//! - A different floor (48 dp): restyle the base with your own
//!   `min_width` / `min_height`. The release overlay still frees it at
//!   `Md` and up.
//!
//! # Using it in a component
//!
//! Call it from the slot's `extend` hook:
//!
//! ```ignore
//! component_styles! {
//!     pub Pager<IdeaThemeRef> {
//!         slot chevron {
//!             extend(chevron_arms)
//!             base(_t) { … }
//!         }
//!     }
//! }
//! fn chevron_arms(s: &mut SlotRestyle<'_, PagerStyles>) {
//!     crate::touch::touch_target(s);
//! }
//! ```
//!
//! Author arms, states and compounds layer over the responsive overlays
//! (`base < breakpoints < containers < author axes < states < compounds`),
//! so a slot must not set `min_width` / `min_height` in a `variant` arm it
//! also wants floored — the arm would win at every width.

use runtime_core::{Breakpoint, ComponentStyles, Length, SlotRestyle, StyleRules, Tokenized};

/// The smallest tap target, in px/pt/dp, applied at the phone breakpoints.
/// Apple HIG 44 pt / WCAG 2.5.5 44 CSS px.
pub const MIN_TOUCH_TARGET_PX: f32 = 44.0;

/// The first breakpoint that drops the floor — `Xs` and `Sm` keep it.
pub const TOUCH_TARGET_RELEASE: Breakpoint = Breakpoint::Md;

/// `min_width` / `min_height` at [`MIN_TOUCH_TARGET_PX`] — the rules
/// [`touch_target`] puts on the slot's base.
pub fn touch_target_rules() -> StyleRules {
    let floor = Tokenized::Literal(Length::Px(MIN_TOUCH_TARGET_PX));
    StyleRules {
        min_width: Some(floor.clone()),
        min_height: Some(floor),
        ..Default::default()
    }
}

/// `min_width` / `min_height` back to `auto` (the initial value) — the
/// release overlay's rules, and the per-instance / app-wide opt-out.
pub fn no_touch_target() -> StyleRules {
    StyleRules {
        min_width: Some(Tokenized::Literal(Length::Auto)),
        min_height: Some(Tokenized::Literal(Length::Auto)),
        ..Default::default()
    }
}

/// Floor `slot` at [`MIN_TOUCH_TARGET_PX`] on phones: the rules on the
/// base, released to `auto` from [`TOUCH_TARGET_RELEASE`] up.
pub fn touch_target<S: ComponentStyles>(slot: &mut SlotRestyle<'_, S>) {
    slot.base(|_| touch_target_rules());
    slot.breakpoint(TOUCH_TARGET_RELEASE, |_| no_touch_target());
}
