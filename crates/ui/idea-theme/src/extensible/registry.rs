//! The tone registry and the composition helpers tone-aware components
//! build their slots with.
//!
//! A tone is a palette ([`Tone`](super::Tone)); a variant is a surface
//! recipe over a tone ([`Variant`](super::Variant)). A component that takes
//! both (Button, Badge, Alert, …) declares `tone` and `variant` axes on its
//! `component_styles!` slots, and its `extend` hook adds one compound per
//! `(tone, variant)` pair with [`tone_variant_arms`] — rendered at
//! composition time from the tones registered HERE. That is what makes an
//! app tone reach every component with no per-component wiring:
//! [`register_tone`] (or `app_theme! { …, tones: [Brand] }`, which
//! installing the theme registers) adds the tone and recomposes every
//! component's slots, in whatever order the app restyles and installs.

use std::cell::RefCell;

use runtime_core::component_styles::{self, ComponentStyles, SlotRestyle};
use runtime_core::StyleRules;

use super::variant::{variant_state_overlay, InteractState};
use super::{ResolutionCtx, ToneRef, Variant};
use crate::theme::IdeaThemeRef;

runtime_tls::thread_local! {
    /// App-registered tones, in registration order (built-ins excluded).
    static TONES: RefCell<Vec<ToneRef>> = const { RefCell::new(Vec::new()) };
}

/// Make `tone` drawable by every tone-aware component. Idempotent by
/// [`key`](super::Tone::key): registering a key twice keeps the first.
/// Registering recomposes every component's slots, so the order relative
/// to restyles and the theme install does not matter — but like a restyle,
/// do it at boot, before the tree is built.
///
/// A tone used without being registered warns in debug builds and draws
/// the component's base.
///
/// # Panics
/// When the key is not a class-safe name (`[a-z0-9_]`): it becomes part
/// of a premint class.
pub fn register_tone(tone: impl Into<ToneRef>) {
    let tone = tone.into();
    let key = tone.key();
    assert!(
        !key.is_empty() && key.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
        "tone key `{key}` must be lowercase ASCII letters, digits and `_` — it becomes part of a class name"
    );
    if is_builtin(key) {
        return;
    }
    let fresh = TONES.with(|t| {
        let mut t = t.borrow_mut();
        if t.iter().any(|x| x.key() == key) {
            false
        } else {
            t.push(tone);
            true
        }
    });
    if fresh {
        component_styles::invalidate_all();
    }
}

/// Register the tones an installed theme brings
/// ([`IdeaTheme::extension_tones`](crate::theme::IdeaTheme::extension_tones)).
pub(crate) fn register_theme_tones(theme: &IdeaThemeRef) {
    use crate::theme::IdeaTheme;
    for t in theme.extension_tones() {
        register_tone(t);
    }
}

fn is_builtin(key: &str) -> bool {
    ToneRef::builtins().iter().any(|(k, _)| *k == key) || super::tone::palette_hues().iter().any(|(k, _)| *k == key)
}

/// The app-registered tones (no built-ins), in registration order.
pub fn registered_tones() -> Vec<ToneRef> {
    TONES.with(|t| t.borrow().clone())
}

/// The seven semantic tones plus every registered one — what interactive
/// tone-aware components (Button, Alert, Switch, …) draw.
pub fn semantic_tones() -> Vec<ToneRef> {
    ToneRef::builtins().into_iter().map(|(_, t)| t).chain(registered_tones()).collect()
}

/// [`semantic_tones`] plus the twelve palette hues — what categorization
/// components (Badge, Tag, Avatar) draw. Kept off the interactive
/// components because each tone is rendered into the build-time CSS for
/// every variant of every component that draws it: the hues on all of them
/// measured 177 KB → 437 KB of CSS for every app.
pub fn categorization_tones() -> Vec<ToneRef> {
    ToneRef::builtins()
        .into_iter()
        .chain(super::tone::palette_hues())
        .map(|(_, t)| t)
        .chain(registered_tones())
        .collect()
}

/// Run `f` with the installed idea theme — or, before one is installed,
/// the default light theme. Slot composition renders tone recipes through
/// it; they produce token references (the theme only supplies fallbacks),
/// so the composed rules and their premint identity are the same whichever
/// theme is active.
pub fn with_idea_theme<R>(f: impl FnOnce(&IdeaThemeRef) -> R) -> R {
    if runtime_core::theme_installed() {
        let theme = runtime_core::active_theme_untracked();
        if let Some(t) = theme.downcast_ref::<IdeaThemeRef>() {
            return f(t);
        }
    }
    f(&IdeaThemeRef::new(crate::theme::light_theme()))
}

/// Add one compound per tone: `tone = <key> ∧ variant = variant_key`,
/// carrying `recipe` rendered over that tone. A component's `extend` hook
/// calls this per built-in variant; an app calls it in a restyle to give a
/// value it [`define`d] a tone-aware look:
///
/// ```ignore
/// const RAISED: ButtonVariant = ButtonVariant::define("raised");
/// ButtonStyles::restyle(|s| {
///     idea_theme::extensible::tone_variant_arms(&mut s.root(), &semantic_tones(), RAISED.key(), &Raised);
/// });
/// ```
///
/// [`define`d]: runtime_core::AxisKey
pub fn tone_variant_arms<S: ComponentStyles>(
    slot: &mut SlotRestyle<'_, S>,
    tones: &[ToneRef],
    variant_key: &str,
    recipe: &dyn Variant,
) {
    with_idea_theme(|theme| {
        for tone in tones {
            let ctx = ResolutionCtx { theme, tone: &*tone.0 };
            slot.raw_compound(&[("tone", tone.key()), ("variant", variant_key)], recipe.render(&ctx));
        }
    });
}

/// Add the per-`(tone, variant)` hover and press feedback of a clickable
/// control: a compound on `tone ∧ variant ∧ __state_hovered` (and
/// `pressed`) carrying [`variant_state_overlay`], so the feedback merges
/// after the surface fill and suits it — a translucent wash on
/// transparent-resting variants, a tone-preserving dim (plus the tone's
/// glow on Filled) on the others.
pub fn tone_variant_state_overlays<S: ComponentStyles>(
    slot: &mut SlotRestyle<'_, S>,
    tones: &[ToneRef],
    variant_key: &str,
) {
    with_idea_theme(|theme| {
        for tone in tones {
            let ctx = ResolutionCtx { theme, tone: &*tone.0 };
            for (state, which) in [("__state_hovered", InteractState::Hover), ("__state_pressed", InteractState::Press)] {
                slot.raw_compound(
                    &[("tone", tone.key()), ("variant", variant_key), (state, "on")],
                    variant_state_overlay(variant_key, &ctx, which),
                );
            }
        }
    });
}

/// Add one arm per tone on the `tone` axis, carrying `f(tone)` — for a
/// slot whose look depends on the tone alone (a checked checkbox's fill, a
/// progress bar).
pub fn tone_arms<S: ComponentStyles>(
    slot: &mut SlotRestyle<'_, S>,
    tones: &[ToneRef],
    f: impl Fn(&ResolutionCtx) -> StyleRules,
) {
    with_idea_theme(|theme| {
        for tone in tones {
            let ctx = ResolutionCtx { theme, tone: &*tone.0 };
            slot.raw_arm("tone", tone.key(), f(&ctx));
        }
    });
}

/// Add one compound per tone on `tone = <key> ∧ <axis> = <value>`,
/// carrying `f(tone)` — tone-dependent rules that apply only under another
/// axis value (an IconButton's selected fill).
pub fn tone_compounds<S: ComponentStyles>(
    slot: &mut SlotRestyle<'_, S>,
    tones: &[ToneRef],
    other: (&str, &str),
    f: impl Fn(&ResolutionCtx) -> StyleRules,
) {
    with_idea_theme(|theme| {
        for tone in tones {
            let ctx = ResolutionCtx { theme, tone: &*tone.0 };
            slot.raw_compound(&[("tone", tone.key()), other], f(&ctx));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extensible::tone;

    #[test]
    fn registering_is_idempotent_by_key_and_skips_builtins() {
        let before = registered_tones().len();
        register_tone(tone::Primary);
        register_tone(tone::Red);
        assert_eq!(registered_tones().len(), before, "built-ins are never re-registered");
        crate::tone! {
            Hype using self, theme {
                key = "hype_registry_test",
                fill_bg = theme.colors().text.clone(),
                fill_fg = theme.colors().background.clone(),
                soft_bg = theme.colors().surface_alt.clone(),
                soft_fg = theme.colors().text.clone(),
                stroke_color = theme.colors().border.clone(),
                stroke_fg = theme.colors().text.clone(),
                ghost_fg = theme.colors().text.clone(),
                disabled = theme.colors().text_muted.clone(),
                focus_ring = theme.colors().focus_ring.clone(),
            }
        }
        register_tone(Hype);
        register_tone(Hype);
        assert_eq!(registered_tones().len(), before + 1);
        assert!(semantic_tones().iter().any(|t| t.key() == "hype_registry_test"));
        assert!(categorization_tones().iter().any(|t| t.key() == "hype_registry_test"));
    }

    #[test]
    #[should_panic(expected = "class name")]
    fn a_key_that_is_not_class_safe_is_rejected() {
        crate::tone! {
            Bad using self, theme {
                key = "Bad Tone",
                fill_bg = theme.colors().text.clone(),
                fill_fg = theme.colors().background.clone(),
                soft_bg = theme.colors().surface_alt.clone(),
                soft_fg = theme.colors().text.clone(),
                stroke_color = theme.colors().border.clone(),
                stroke_fg = theme.colors().text.clone(),
                ghost_fg = theme.colors().text.clone(),
                disabled = theme.colors().text_muted.clone(),
                focus_ring = theme.colors().focus_ring.clone(),
            }
        }
        register_tone(Bad);
    }
}
