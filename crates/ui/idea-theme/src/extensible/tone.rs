//! Built-in tones. Each tone is a zero-sized marker struct; call sites
//! use the type name as the value.
//!
//! - **Semantic tones** (`Primary`, `Danger`, …) route through the
//!   `IdeaTheme.intents()` block — they mean something.
//! - **Palette tones** (`Red`, `Blue`, `Violet`, … — twelve hues) route
//!   through `IdeaTheme.palette()` — plain colors for categorization.
//!   [`palette_hues`] lists them in order, and [`palette_from_seed`]
//!   picks one deterministically from a string (a stable avatar color
//!   per user name).
//!
//! ```ignore
//! use idea_ui::tone;
//!
//! Button(tone = tone::Primary, ...);
//! Button(tone = tone::Danger, ...);
//! ```
//!
//! Apps add custom tones by implementing [`super::Tone`] on a marker
//! struct of their own. The convention is to place them in a `tone`
//! module within the app that `pub use idea_ui::tone::*`
//! so call sites uniformly read `tone::Name` regardless of origin.

use runtime_core::{Color, IdealystSchema, Tokenized};

use super::Tone;
use crate::theme::IdeaTheme;

// Helper — pulls one slot off an `IntentColors` block. The built-ins
// differ only in which block they target: `$group` is the theme getter
// (`intents` for the seven semantic tones, `palette` for the twelve hues)
// and `$block` the field on it.
macro_rules! builtin_tone {
    ($(#[$doc:meta])* $name:ident, $key:literal, $group:ident . $block:ident) => {
        builtin_tone!($(#[$doc])* $name, $key, $group . $block, |_theme| None);
    };
    ($(#[$doc:meta])* $name:ident, $key:literal, $group:ident . $block:ident, glow = $glow:ident) => {
        builtin_tone!(
            $(#[$doc])* $name, $key, $group . $block,
            |theme| Some(theme.effects().$glow.clone())
        );
    };
    ($(#[$doc:meta])* $name:ident, $key:literal, $group:ident . $block:ident, |$theme:ident| $glow:expr) => {
        $(#[$doc])*
        #[derive(Copy, Clone, Default, IdealystSchema)]
        #[schema(value_of = "ToneRef", via = "idea_ui::tone")]
        pub struct $name;

        crate::__remote_marker!(super::ToneRef, $name);

        impl Tone for $name {
            fn key(&self) -> &'static str {
                $key
            }
            fn fill_bg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.$group().$block.solid_bg.clone()
            }
            fn fill_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.$group().$block.solid_text.clone()
            }
            fn soft_bg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.$group().$block.soft_bg.clone()
            }
            fn soft_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.$group().$block.soft_text.clone()
            }
            fn stroke_color(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.$group().$block.border.clone()
            }
            fn stroke_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.$group().$block.fg.clone()
            }
            fn ghost_fg(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.$group().$block.fg.clone()
            }
            fn disabled(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.colors().text_muted.clone()
            }
            fn focus_ring(&self, theme: &dyn IdeaTheme) -> Tokenized<Color> {
                theme.colors().focus_ring.clone()
            }
            #[allow(unused_variables)]
            fn glow(&self, $theme: &dyn IdeaTheme) -> Option<Tokenized<Color>> {
                $glow
            }
        }

        // Reactive-prop coercion: `tone = tone::Primary` into a
        // `#[props]`-wrapped `Reactive<ToneRef>` field. The marker → ref →
        // `Reactive` chain can't go through one `.into()` (the compiler
        // won't chain two `From` impls).
        impl ::core::convert::From<$name> for ::runtime_core::Reactive<super::ToneRef> {
            fn from(marker: $name) -> Self {
                ::runtime_core::Reactive::Static(super::ToneRef::from(marker))
            }
        }

        // Optional-tone coercion: `tone = tone::Danger` into an
        // `Option<ToneRef>` prop (`#[props]` wraps it as
        // `Reactive<Option<ToneRef>>`) with no `Some(...)`. Orphan-legal
        // because the marker — the trait's type parameter — is local: only
        // a BLANKET `impl<T: Tone> From<T> for Option<ToneRef>` is barred.
        // See the note after `ToneRef::builtins` in `extensible/mod.rs`.
        impl ::core::convert::From<$name> for ::core::option::Option<super::ToneRef> {
            fn from(marker: $name) -> Self {
                ::core::option::Option::Some(super::ToneRef::from(marker))
            }
        }
        impl ::core::convert::From<$name>
            for ::runtime_core::Reactive<::core::option::Option<super::ToneRef>>
        {
            fn from(marker: $name) -> Self {
                ::runtime_core::Reactive::Static(::core::option::Option::Some(
                    super::ToneRef::from(marker),
                ))
            }
        }
    };
}

builtin_tone!(/** Built-in semantic tone. */ Primary, "primary", intents.primary, glow = glow_primary);
builtin_tone!(/** Built-in semantic tone. */ Secondary, "secondary", intents.secondary, glow = glow_secondary);
builtin_tone!(/** Built-in semantic tone. */ Neutral, "neutral", intents.neutral, glow = glow_neutral);
builtin_tone!(/** Built-in semantic tone. */ Success, "success", intents.success, glow = glow_success);
builtin_tone!(/** Built-in semantic tone. */ Danger, "danger", intents.danger, glow = glow_danger);
builtin_tone!(/** Built-in semantic tone. */ Warning, "warning", intents.warning, glow = glow_warning);
builtin_tone!(/** Built-in semantic tone. */ Info, "info", intents.info, glow = glow_info);

// The twelve palette hues — color with no semantic meaning, for
// categorization (avatars, user-colored tags, per-project badges). They
// read `theme.palette()`, so a theme retinting `danger` leaves `tone::Red`
// alone. See `crate::theme::Palette`.
builtin_tone!(/** Palette hue — no semantic meaning. */ Red, "red", palette.red);
builtin_tone!(/** Palette hue — no semantic meaning. */ Orange, "orange", palette.orange);
builtin_tone!(/** Palette hue — no semantic meaning. */ Amber, "amber", palette.amber);
builtin_tone!(/** Palette hue — no semantic meaning. */ Yellow, "yellow", palette.yellow);
builtin_tone!(/** Palette hue — no semantic meaning. */ Lime, "lime", palette.lime);
builtin_tone!(/** Palette hue — no semantic meaning. */ Green, "green", palette.green);
builtin_tone!(/** Palette hue — no semantic meaning. */ Teal, "teal", palette.teal);
builtin_tone!(/** Palette hue — no semantic meaning. */ Cyan, "cyan", palette.cyan);
builtin_tone!(/** Palette hue — no semantic meaning. */ Blue, "blue", palette.blue);
builtin_tone!(/** Palette hue — no semantic meaning. */ Indigo, "indigo", palette.indigo);
builtin_tone!(/** Palette hue — no semantic meaning. */ Violet, "violet", palette.violet);
builtin_tone!(/** Palette hue — no semantic meaning. */ Pink, "pink", palette.pink);

/// The twelve palette tones in hue order (red → pink), as `(key, ref)`
/// pairs — the same shape as [`ToneRef::builtins`](super::ToneRef::builtins).
pub fn palette_hues() -> Vec<(&'static str, super::ToneRef)> {
    use super::ToneRef;
    vec![
        ("red", ToneRef::from(Red)),
        ("orange", ToneRef::from(Orange)),
        ("amber", ToneRef::from(Amber)),
        ("yellow", ToneRef::from(Yellow)),
        ("lime", ToneRef::from(Lime)),
        ("green", ToneRef::from(Green)),
        ("teal", ToneRef::from(Teal)),
        ("cyan", ToneRef::from(Cyan)),
        ("blue", ToneRef::from(Blue)),
        ("indigo", ToneRef::from(Indigo)),
        ("violet", ToneRef::from(Violet)),
        ("pink", ToneRef::from(Pink)),
    ]
}

/// Pick a palette tone deterministically from `seed` — the same seed gives
/// the same hue on every run, platform and build. The standard use is
/// giving each user a stable avatar color from their name or id:
///
/// ```ignore
/// Avatar(initials = "NM", tone = tone::palette_from_seed(&user.id))
/// ```
///
/// Hashes with 64-bit FNV-1a rather than `std`'s `DefaultHasher`, whose
/// output is explicitly not guaranteed stable across Rust releases — a
/// toolchain bump would otherwise recolor every avatar in the app.
pub fn palette_from_seed(seed: &str) -> super::ToneRef {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let hash = seed
        .as_bytes()
        .iter()
        .fold(FNV_OFFSET, |h, b| (h ^ u64::from(*b)).wrapping_mul(FNV_PRIME));
    let mut hues = palette_hues();
    let index = (hash % hues.len() as u64) as usize;
    hues.swap_remove(index).1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::{dark_theme, light_theme, IdeaThemeDefaults, IdeaThemeRef};
    use runtime_core::color::{parse, Rgba};

    /// A palette tone reads the theme's `palette` block, never `intents`:
    /// every slot comes back under its `palette-<hue>-<slot>` token. This
    /// is what keeps a theme's `danger` retint from dragging `tone::Red`
    /// (and every red avatar/tag) along with it.
    #[test]
    fn palette_tones_read_the_palette_block_not_intents() {
        let mut t = light_theme();
        t.intents.danger.solid_bg = Tokenized::Literal(Color("#000000".into()));
        let theme = IdeaThemeRef::new(t);
        assert_eq!(Red.fill_bg(&theme).name(), Some("palette-red-solid-bg"));
        assert_eq!(Red.fill_bg(&theme).value().0, "#dc2626", "danger retint must not reach tone::Red");
        assert_eq!(Blue.fill_fg(&theme).name(), Some("palette-blue-solid-text"));
        assert_eq!(Blue.soft_bg(&theme).name(), Some("palette-blue-soft-bg"));
        assert_eq!(Blue.soft_fg(&theme).name(), Some("palette-blue-soft-text"));
        assert_eq!(Blue.stroke_color(&theme).name(), Some("palette-blue-border"));
        assert_eq!(Blue.stroke_fg(&theme).name(), Some("palette-blue-fg"));
    }

    /// `palette_hues` lists all twelve hues, in `PALETTE_NAMES` order, each
    /// keyed by its own tone key.
    #[test]
    fn palette_hues_cover_every_palette_name_in_order() {
        let keys: Vec<&str> = palette_hues().iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, crate::theme::PALETTE_NAMES);
        for (key, tone) in palette_hues() {
            assert_eq!(tone.key(), key);
        }
    }

    /// Pinned outputs. `palette_from_seed` is a persistence contract — an
    /// app's users keep their avatar color across releases — so the hash
    /// must never change silently. If this fails, every existing user just
    /// changed color.
    #[test]
    fn palette_from_seed_is_pinned() {
        let picks: Vec<&str> = ["", "ada@idealyst.dev", "grace@idealyst.dev", "user-42", "Nicho"]
            .iter()
            .map(|s| palette_from_seed(s).key())
            .collect();
        assert_eq!(picks, PINNED_SEED_PICKS);
    }
    const PINNED_SEED_PICKS: [&str; 5] = ["green", "pink", "cyan", "yellow", "blue"];

    /// Seeds spread over the whole palette (a hash that collapsed onto a
    /// few hues would defeat the point of having twelve).
    #[test]
    fn palette_from_seed_reaches_every_hue() {
        let mut seen = std::collections::BTreeSet::new();
        for i in 0..500 {
            seen.insert(palette_from_seed(&format!("user-{i}")).key());
        }
        assert_eq!(seen.len(), 12, "only reached {seen:?}");
    }

    fn relative_luminance(c: Rgba) -> f64 {
        let lin = |v: u8| {
            let s = f64::from(v) / 255.0;
            if s <= 0.04045 { s / 12.92 } else { ((s + 0.055) / 1.055).powf(2.4) }
        };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    /// `fg` alpha-composited over `bg` (bg opaque).
    fn over(fg: Rgba, bg: Rgba) -> Rgba {
        let a = f64::from(fg.a) / 255.0;
        let mix = |f: u8, b: u8| (f64::from(f) * a + f64::from(b) * (1.0 - a)).round() as u8;
        Rgba::new(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b), 255)
    }

    fn contrast(a: Rgba, b: Rgba) -> f64 {
        let (la, lb) = (relative_luminance(a), relative_luminance(b));
        (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
    }

    /// The contract an Avatar's initials (and any text on a palette fill)
    /// relies on: in both themes, every hue's `solid_text` on `solid_bg`
    /// and `soft_text` on `soft_bg` meets WCAG AA for normal text (4.5:1).
    /// Translucent soft fills (dark theme) are composited over the theme's
    /// surface first — that is what the eye actually sees.
    #[test]
    fn palette_text_pairs_meet_wcag_aa() {
        fn check(label: &str, theme: IdeaThemeDefaults) {
            let surface = parse(&theme.colors.surface.value().0).unwrap();
            let p = &theme.palette;
            let mut failures = Vec::new();
            for (hue, block) in crate::theme::PALETTE_NAMES.iter().zip([
                &p.red, &p.orange, &p.amber, &p.yellow, &p.lime, &p.green,
                &p.teal, &p.cyan, &p.blue, &p.indigo, &p.violet, &p.pink,
            ]) {
                for (pair, text, bg) in [
                    ("solid", &block.solid_text, &block.solid_bg),
                    ("soft", &block.soft_text, &block.soft_bg),
                ] {
                    let bg = over(parse(&bg.value().0).unwrap(), surface);
                    let text = over(parse(&text.value().0).unwrap(), bg);
                    let ratio = contrast(text, bg);
                    if ratio < 4.5 {
                        failures.push(format!("{label} {hue} {pair}: {ratio:.2}"));
                    }
                }
            }
            assert!(failures.is_empty(), "below WCAG AA 4.5:1 — {failures:#?}");
        }
        check("light", light_theme());
        check("dark", dark_theme());
    }

    /// The accent gradient's text must read on every stop, in both
    /// built-in themes — it is drawn across the whole fill.
    #[test]
    fn effect_gradient_text_meets_wcag_aa() {
        for (label, theme) in [("light", light_theme()), ("dark", dark_theme())] {
            let e = &theme.effects;
            let text = parse(&e.gradient_text.value().0).unwrap();
            for (stop, c) in [
                ("start", &e.gradient_start),
                ("mid", &e.gradient_mid),
                ("end", &e.gradient_end),
            ] {
                let ratio = contrast(text, parse(&c.value().0).unwrap());
                assert!(ratio >= 4.5, "{label} gradient {stop}: {ratio:.2} < 4.5");
            }
        }
    }
}
