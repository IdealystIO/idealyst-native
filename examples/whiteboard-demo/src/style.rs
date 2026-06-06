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

/// Resolve an idea-ui NEUTRAL token (`surface`, `text`, `border`, …) to a
/// concrete [`Color`] NOW. Call inside a `reactive_style` closure: reading the
/// active theme subscribes the surrounding reactive scope, so the value
/// re-resolves automatically on a light/dark swap. This is how the board chrome
/// follows the app theme without per-color plumbing.
pub fn token(getter: impl Fn(&idea_ui::Colors) -> Tokenized<Color> + 'static) -> Color {
    idea_ui::idea_color(getter)()
}

/// Like [`token`] but applies `alpha` (0..=1) to the resolved color, returning a
/// CSS `rgba(...)` string. The floating chrome's surfaces are frosted
/// (translucent over the canvas); opaque tokens would lose that, so we resolve
/// the themed color and re-attach the alpha.
pub fn token_alpha(
    getter: impl Fn(&idea_ui::Colors) -> Tokenized<Color> + 'static,
    alpha: f32,
) -> Color {
    let c = token(getter);
    let rgba = runtime_core::color::parse_or(&c.0, runtime_core::color::Rgba::new(255, 255, 255, 255));
    Color(format!(
        "rgba({},{},{},{:.3})",
        rgba.r,
        rgba.g,
        rgba.b,
        alpha.clamp(0.0, 1.0)
    ))
}

/// Resolve an idea-ui INTENT token (`primary`, `danger`, …) to a concrete
/// [`Color`] NOW. Mirrors [`token`] but reaches the active theme's `Intents`
/// rather than its neutrals — used for the accent on selected tool-rail dots,
/// which should track the brand color across themes.
pub fn token_intent(
    getter: impl Fn(&idea_ui::Intents) -> Tokenized<Color> + 'static,
) -> Color {
    let theme = idea_ui::active_theme();
    let idea = theme
        .downcast_ref::<idea_ui::IdeaThemeRef>()
        .expect("token_intent: active theme is not an IdeaThemeRef — call install_idea_theme(...) first");
    getter(idea.inner().intents()).value().clone()
}

/// A static style source from a finished `StyleRules` literal.
pub fn static_style(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

/// Mount `child` only while the board is the active route (`focused`).
///
/// The HIDE is INSTANT (no exit animation) on purpose: when a screen is pushed,
/// the chrome must vanish in the same synchronous turn as the navigation commit,
/// BEFORE the next paint — otherwise an exit fade leaves the toolbar briefly
/// visible over the incoming screen / sliding transition. The RETURN fades in (a
/// screen pop reveals the board), and each dock's own inner `presence` still
/// animates its state toggles.
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

/// Equal border on all sides, from an already-resolved [`Color`] (e.g. a theme
/// token via `idea_color`) rather than a CSS string.
pub fn border_all_color(px: f32, color: Color) -> StyleRules {
    let c = Tokenized::Literal(color);
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
