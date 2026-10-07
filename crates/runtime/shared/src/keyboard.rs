//! Soft keyboard — how much of the app the on-screen keyboard covers, how
//! the platform animates it, and how a `keyboard_avoiding_view` stays clear
//! of it.
//!
//! ## Nothing avoids the keyboard unless asked
//!
//! The app viewport never changes for the keyboard. A
//! `keyboard_avoiding_view` ([`KeyboardAvoid`]) measures how much of ITS OWN
//! box the keyboard covers and keeps its content clear, moving in step with
//! the platform's own keyboard animation:
//!
//! - **iOS** — the avoider's change (padding → layout pass, or a lift in
//!   its transform) runs inside a `UIView` animation block carrying the
//!   keyboard notification's own duration and curve, so Core Animation
//!   interpolates it with the keyboard's exact timing. No per-frame work.
//! - **Android** — the system IME animation's per-frame progress
//!   (`WindowInsetsAnimationCompat`) drives `translationY` writes in
//!   Kotlin; a `Padding` avoider lays out once per keyboard move.
//! - **Web (mobile browsers)** — browsers expose the keyboard's size but not
//!   its animation, so avoiders ride a CSS transition with a best-estimate
//!   timing ([`WEB_KEYBOARD_ESTIMATE`]).
//!
//! Wrap the app root to avoid it everywhere; wrap a portal's content
//! (modal, sheet) separately — portals mount outside the root. Inside a
//! `Padding` avoider, `.safe_area(BOTTOM)` insets collapse under the
//! keyboard ([`bottom_inset_under_keyboard`]).
//!
//! ## Author surface
//!
//! [`KeyboardInset`] is the value behind the author-facing
//! `keyboard_inset()` signal (the per-world reactive ctx lives in
//! `runtime_vocabulary::keyboard`): the inset the keyboard is animating TO
//! plus that animation's timing as a [`Transition`], for app code that
//! reacts to the keyboard itself.
//!
//! [`Transition`]: crate::Transition

use crate::style::{Easing, Transition};

/// The soft keyboard's coverage of the app's root view, in the same
/// logical unit the style system uses (points / dp / CSS px).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct KeyboardInset {
    /// How much of the root's bottom edge the keyboard covers once its
    /// current animation settles. `0.0` when the keyboard is hidden (or
    /// docked beside the app rather than over it).
    pub height: f32,
    /// The platform keyboard's own timing for the move to `height` —
    /// duration and curve as reported by the platform where it exposes
    /// them (iOS, Android), a best estimate on web. Use it to animate
    /// author-side reactions in step with the keyboard.
    pub transition: Transition,
}

impl KeyboardInset {
    /// Keyboard hidden, nothing animating.
    pub const HIDDEN: Self = Self {
        height: 0.0,
        transition: Transition { duration_ms: 0, easing: Easing::Linear },
    };

    /// A keyboard covering `height` (negative values clamp to zero),
    /// reached with `transition`.
    pub fn new(height: f32, transition: Transition) -> Self {
        Self { height: height.max(0.0), transition }
    }

    /// Whether the keyboard covers any of the root.
    pub fn is_visible(&self) -> bool {
        self.height > 0.0
    }
}

impl Default for KeyboardInset {
    fn default() -> Self {
        Self::HIDDEN
    }
}

/// UIKit's keyboard animation curve (`UIKeyboardAnimationCurveUserInfoKey`
/// = 7, a private curve outside the public `UIViewAnimationCurve` enum)
/// as a cubic-bezier. The iOS backend animates with the real curve; this
/// approximation is what [`KeyboardInset::transition`] hands author code,
/// and what the web backend uses for Safari-style keyboards.
pub const IOS_KEYBOARD_EASING: Easing = Easing::CubicBezier(0.38, 0.7, 0.125, 1.0);

/// Android's IME inset interpolator — `InsetsController`'s
/// `SYNC_IME_INTERPOLATOR`, `PathInterpolator(0.2, 0, 0, 1)`. The
/// Android backend reports the system animation's real per-frame
/// progress; this curve is what [`KeyboardInset::transition`] carries
/// alongside the real duration.
pub const ANDROID_IME_EASING: Easing = Easing::CubicBezier(0.2, 0.0, 0.0, 1.0);

/// iOS keyboard show/hide duration in milliseconds (UIKit has reported
/// 0.25 s in `UIKeyboardAnimationDurationUserInfoKey` since iOS 7). Only
/// the web backend uses it, as the estimate for Safari, which exposes no
/// keyboard timing; the iOS backend reads the real value per
/// notification.
pub const IOS_KEYBOARD_DURATION_MS: u32 = 250;

/// Android's IME show duration (`InsetsController`'s
/// `ANIMATION_DURATION_SYNC_IME_MS`). Web estimate for Chromium-based
/// mobile browsers only; the Android backend reads each animation's real
/// `durationMillis`.
pub const ANDROID_IME_DURATION_MS: u32 = 285;

/// The web backend's estimate of the keyboard animation when the browser
/// exposes the VirtualKeyboard API (Chromium on Android).
pub const WEB_KEYBOARD_ESTIMATE_CHROMIUM: Transition =
    Transition { duration_ms: ANDROID_IME_DURATION_MS, easing: ANDROID_IME_EASING };

/// The web backend's estimate of the keyboard animation everywhere else
/// (Safari on iOS / iPadOS, which only reports through `visualViewport`).
pub const WEB_KEYBOARD_ESTIMATE: Transition =
    Transition { duration_ms: IOS_KEYBOARD_DURATION_MS, easing: IOS_KEYBOARD_EASING };

/// How a `keyboard_avoiding_view` keeps its content clear of the soft
/// keyboard. The view measures how much of ITS OWN box the keyboard
/// covers (so a view that ends above a tab bar only moves by what it
/// actually overlaps) and applies that amount one of two ways.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub enum KeyboardAvoidBehavior {
    /// Pad the view's bottom by the covered height: its content area
    /// shrinks to end at the keyboard's top edge (a scroll area above a
    /// composer gets shorter; the composer sits on the keyboard). Bottom
    /// safe-area insets inside the view collapse under the keyboard.
    #[default]
    Padding,
    /// Lift the whole view by the covered height without re-laying out
    /// anything — the cheapest mode. For forms, sheets and modals whose
    /// content can simply move up.
    Translate,
}

/// A `keyboard_avoiding_view`'s configuration.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "remote-serde", derive(serde::Serialize, serde::Deserialize))]
pub struct KeyboardAvoid {
    pub behavior: KeyboardAvoidBehavior,
    /// `true` (default): move in step with the platform's own keyboard
    /// animation. `false`: jump straight to the final position when the
    /// keyboard starts to move.
    pub animated: bool,
}

impl Default for KeyboardAvoid {
    fn default() -> Self {
        Self { behavior: KeyboardAvoidBehavior::Padding, animated: true }
    }
}

/// How much of a view the keyboard covers, from the view's bottom edge and
/// the keyboard's top edge in the same vertical coordinate space (y grows
/// downward). Zero when the keyboard is hidden or entirely below the view.
pub fn keyboard_overlap(view_bottom: f32, keyboard_top: f32) -> f32 {
    (view_bottom - keyboard_top).max(0.0)
}

/// The bottom safe-area inset still in effect while the keyboard covers
/// `keyboard` of the root's bottom edge. The keyboard rises from the
/// bottom of the screen, so it hides the region the bottom safe area
/// reserves (home indicator, gesture pill, navigation bar) first; only
/// the part the keyboard does NOT cover remains.
pub fn bottom_inset_under_keyboard(safe_bottom: f32, keyboard: f32) -> f32 {
    (safe_bottom - keyboard.max(0.0)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_is_default_and_not_visible() {
        assert_eq!(KeyboardInset::default(), KeyboardInset::HIDDEN);
        assert!(!KeyboardInset::HIDDEN.is_visible());
        assert_eq!(KeyboardInset::HIDDEN.transition.duration_ms, 0);
    }

    #[test]
    fn new_clamps_negative_height() {
        let k = KeyboardInset::new(-12.0, WEB_KEYBOARD_ESTIMATE);
        assert_eq!(k.height, 0.0);
        assert!(!k.is_visible());
        let k = KeyboardInset::new(336.0, WEB_KEYBOARD_ESTIMATE);
        assert_eq!(k.height, 336.0);
        assert!(k.is_visible());
    }

    #[test]
    fn overlap_is_the_part_of_the_view_below_the_keyboard_top() {
        // Full-screen view (bottom 852), keyboard top at 516: 336 covered.
        assert_eq!(keyboard_overlap(852.0, 516.0), 336.0);
        // A view ending above a tab bar (bottom 769) only moves by what it
        // actually overlaps.
        assert_eq!(keyboard_overlap(769.0, 516.0), 253.0);
        // Keyboard hidden (top at/below the view) → nothing.
        assert_eq!(keyboard_overlap(852.0, 852.0), 0.0);
        assert_eq!(keyboard_overlap(400.0, 516.0), 0.0);
    }

    #[test]
    fn keyboard_avoid_defaults_to_animated_padding() {
        let d = KeyboardAvoid::default();
        assert_eq!(d.behavior, KeyboardAvoidBehavior::Padding);
        assert!(d.animated);
    }

    #[test]
    fn bottom_safe_inset_collapses_under_the_keyboard() {
        // No keyboard: the full home-indicator inset applies.
        assert_eq!(bottom_inset_under_keyboard(34.0, 0.0), 34.0);
        // A keyboard taller than the inset hides it completely — no gap
        // between the content and the keyboard's top edge.
        assert_eq!(bottom_inset_under_keyboard(34.0, 336.0), 0.0);
        // Mid-animation (keyboard only 20 pt up): the uncovered 14 pt of
        // the inset still applies, so the content rides up continuously
        // instead of jumping by the inset when the keyboard starts.
        assert_eq!(bottom_inset_under_keyboard(34.0, 20.0), 14.0);
        // A bogus negative keyboard report never widens the inset.
        assert_eq!(bottom_inset_under_keyboard(34.0, -5.0), 34.0);
    }
}
