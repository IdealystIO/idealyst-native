//! Shared style helpers used across the board chrome and the navigator
//! screens. These are intentionally plain functions (not components):
//! they build / compose `StyleRules` and wrap reactive style sources, the
//! raw material that the `#[component]` bodies feed into `ui!`'s
//! `style = …` slots.

use runtime_core::{
    presence, Color, Easing, Element, IntoElement, Length, PresenceAnim, PresenceState,
    StyleApplication, StyleRules, StyleSheet, Tokenized,
};
use std::rc::Rc;

/// Wrap a signal-reading `StyleRules` builder into a REACTIVE style source.
/// `.with_style(Rc<StyleSheet>)` resolves once and memoizes; a closure
/// `Fn() -> StyleApplication` re-runs whenever a signal it reads changes.
pub fn reactive_style(f: impl Fn() -> StyleRules + 'static) -> impl Fn() -> StyleApplication {
    move || StyleApplication::new(Rc::new(StyleSheet::r#static(f())))
}

/// A static style source from a finished `StyleRules` literal.
pub fn static_style(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

/// Mount `child` only while the board is the active route (`focused`).
///
/// The HIDE is INSTANT (no exit animation) on purpose: when a screen is pushed,
/// the chrome must vanish in the same synchronous turn as the navigation commit,
/// BEFORE the next paint — otherwise an exit fade leaves the capture-excluded
/// toolbar briefly visible over the incoming screen / sliding transition. The
/// RETURN fades in (a screen pop reveals the board), and each dock's own inner
/// `presence` still animates its state toggles. `presence` mount/unmount — not a
/// reactive resize — is what works in the detached `PrivateLayer` window.
pub fn focus_gate(
    focused: Rc<dyn Fn() -> bool>,
    child: impl Fn() -> Element + 'static,
) -> Element {
    presence(child)
        .present(move || focused())
        .enter(PresenceAnim::new(
            PresenceState { opacity: Some(0.0), ..Default::default() },
            140,
            Easing::EaseOut,
        ))
        .into_element()
}

/// Shorthand: equal border-radius on all corners.
pub fn radius(px: f32) -> StyleRules {
    StyleRules {
        border_top_left_radius: Some(Length::Px(px).into()),
        border_top_right_radius: Some(Length::Px(px).into()),
        border_bottom_left_radius: Some(Length::Px(px).into()),
        border_bottom_right_radius: Some(Length::Px(px).into()),
        ..Default::default()
    }
}

/// Equal border on all sides.
pub fn border_all(px: f32, color: &str) -> StyleRules {
    let c = Tokenized::Literal(Color(color.to_string()));
    StyleRules {
        border_top_width: Some(px.into()),
        border_bottom_width: Some(px.into()),
        border_left_width: Some(px.into()),
        border_right_width: Some(px.into()),
        border_top_color: Some(c.clone()),
        border_bottom_color: Some(c.clone()),
        border_left_color: Some(c.clone()),
        border_right_color: Some(c),
        ..Default::default()
    }
}

/// Overlay `extra`'s set fields onto `base`. Lets the radius/border shorthands
/// compose with a base `StyleRules` literal.
pub fn merge(base: &mut StyleRules, extra: StyleRules) {
    macro_rules! take {
        ($($f:ident),* $(,)?) => { $( if extra.$f.is_some() { base.$f = extra.$f; } )* };
    }
    take!(
        border_top_left_radius,
        border_top_right_radius,
        border_bottom_left_radius,
        border_bottom_right_radius,
        border_top_width,
        border_bottom_width,
        border_left_width,
        border_right_width,
        border_top_color,
        border_bottom_color,
        border_left_color,
        border_right_color,
    );
}

/// Compose a base `StyleRules` with the radius / border shorthands and return
/// the finished value — the common `let mut s = …; merge(&mut s, radius(..)); s`
/// shape collapsed to one expression.
pub fn styled(mut base: StyleRules, extras: impl IntoIterator<Item = StyleRules>) -> StyleRules {
    for e in extras {
        merge(&mut base, e);
    }
    base
}
