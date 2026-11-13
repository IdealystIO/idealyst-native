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
//! The release is `Md` by default, not `Sm`: the `Sm` band (640–767) is
//! large phones in landscape and small tablets — still fingers. Width is
//! the only signal the style system can branch on; the primary pointer
//! kind is a runtime signal (`runtime_core::primary_pointer`), not an
//! overlay axis.
//!
//! An app whose own layout switches at another breakpoint moves the
//! release with [`set_touch_target_release`], once at boot, so every
//! floored control (and any that gains the floor later) steps at the same
//! width as the rest of the app.
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
//!   [`touch_target_release`] and up.
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

/// The default first breakpoint that drops the floor — `Xs` and `Sm` keep
/// it. [`set_touch_target_release`] moves it.
pub const TOUCH_TARGET_RELEASE: Breakpoint = Breakpoint::Md;

runtime_tls::thread_local! {
    static RELEASE: std::cell::Cell<Breakpoint> = const { std::cell::Cell::new(TOUCH_TARGET_RELEASE) };
}

/// The breakpoint that currently drops the floor: [`TOUCH_TARGET_RELEASE`]
/// unless the app moved it.
pub fn touch_target_release() -> Breakpoint {
    RELEASE.with(|r| r.get())
}

/// Release the touch floor from `at` up, instead of `Md`, on every
/// component that floors a slot. Call it once at boot, beside
/// `install_theme`: `extend` hooks read it when a component's styles are
/// composed, so this drops every composition (a change after the first
/// mount restyles mounted controls the way a late restyle does, with the
/// same warning).
///
/// `Breakpoint::Xs` releases it at every width, which is the floor off.
pub fn set_touch_target_release(at: Breakpoint) {
    if RELEASE.with(|r| r.replace(at)) != at {
        runtime_core::component_styles::invalidate_all();
    }
}

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
/// base, released to `auto` from [`touch_target_release`] up.
pub fn touch_target<S: ComponentStyles>(slot: &mut SlotRestyle<'_, S>) {
    slot.base(|_| touch_target_rules());
    slot.breakpoint(touch_target_release(), |_| no_touch_target());
}
