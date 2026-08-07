//! `Card` — surface container, built on the extensible Variant trait.
//!
//! ```ignore
//! ui! {
//!     Card(variant = card::variant::Elevated, padding = CardPadding::Md) {
//!         Typography(content = "Stats", kind = typography_kind::H2)
//!     }
//! }
//! ```
//!
//! Two built-in variants: [`variant::Flat`] (surface bg) and
//! [`variant::Elevated`] (surface-alt bg + drop shadow). They read the
//! theme's surface colors directly — no intent palette — so they ignore
//! the `tone` field of `ResolutionCtx`.
//!
//! The Card stylesheet is built programmatically (variant × padding
//! axes) and installed lazily on first use. Apps with custom Card
//! variants install an extended sheet via [`install_card_sheet`]
//! before mounting.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{
    component, ui, ChildList, Easing, IdealystSchema, Length, Element, Overflow, Reactive,
    StyleApplication, StyleRules, StyleSheet, Tokenized, Transition, VariantEnum, VariantSet,
};

use idea_theme::active_theme;
use idea_theme::extensible::{
    premint_identity, tone as tones, variant_keys, RefBuiltins, ResolutionCtx, ToneRef,
    VariantRef,
};
use idea_theme::theme::IdeaThemeRef;

use crate::slot_override::apply_override;

pub use crate::stylesheets::CardPadding;
use idea_theme::tokens;

/// Built-in Card variants. Card's variants don't consume a Tone (a
/// surface container isn't intent-colored) — they read the theme's
/// surface colors directly via `ctx.theme.colors()`.
pub mod variant {
    use idea_theme::extensible::{ResolutionCtx, Variant, VariantRef};
    use runtime_core::{Color, StyleRules};

    // Reactive-prop coercion for the card-local variants, so a bare marker
    // (`variant = variant::Flat`) coerces into a `#[props]`-wrapped
    // `Reactive<VariantRef>` field. Hand-written markers don't go through the
    // `variant!` macro, so they emit it here (see idea-theme's `variant!`).
    macro_rules! card_variant_reactive {
        ($($name:ident),*) => { $(
            impl ::core::convert::From<$name> for ::runtime_core::Reactive<VariantRef> {
                fn from(marker: $name) -> Self {
                    ::runtime_core::Reactive::Static(VariantRef::from(marker))
                }
            }
        )* };
    }

    /// Flat — page-surface background, no shadow.
    #[derive(Copy, Clone, Default)]
    pub struct Flat;

    impl Variant for Flat {
        fn key(&self) -> &'static str {
            "flat"
        }
        fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
            StyleRules {
                background: Some(ctx.theme.colors().surface.clone()),
                ..Default::default()
            }
        }
    }

    /// Elevated — raised surface with a soft drop shadow. Uses
    /// `surface_alt` so the card reads as a layer above the page's
    /// `surface`, distinct even on platforms that don't render shadows.
    #[derive(Copy, Clone, Default)]
    pub struct Elevated;

    impl Variant for Elevated {
        fn key(&self) -> &'static str {
            "elevated"
        }
        fn render(&self, ctx: &ResolutionCtx) -> StyleRules {
            StyleRules {
                background: Some(ctx.theme.colors().surface_alt.clone()),
                shadow: Some(runtime_core::Shadow {
                    x: 0.0,
                    y: 4.0,
                    blur: 16.0,
                    color: Color("rgba(15, 17, 21, 0.10)".into()),
                }),
                ..Default::default()
            }
        }
    }

    card_variant_reactive!(Flat, Elevated);
}

thread_local! {
    static CARD_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
}

/// Install a custom Card stylesheet (e.g. with app-defined variants).
/// Call before the first Card mounts. If never called, the default
/// sheet (Flat + Elevated variants) is installed lazily on first use.
pub fn install_card_sheet(sheet: Rc<StyleSheet>) {
    CARD_SHEET.with(|s| *s.borrow_mut() = Some(sheet));
}

fn card_sheet() -> Rc<StyleSheet> {
    CARD_SHEET.with(|s| {
        if s.borrow().is_none() {
            let tones: Vec<ToneRef> = ToneRef::builtins().into_iter().map(|(_, t)| t).collect();
            let built =
                build_card_sheet(vec![variant::Flat.into(), variant::Elevated.into()], tones);
            *s.borrow_mut() = Some(built);
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

/// Build a Card stylesheet from a list of variants. The padding axis
/// is fixed (none/sm/md/lg → theme spacing tokens). Each variant arm
/// pulls its background/shadow from `variant.render(ctx)` (Card
/// variants ignore the tone, so a placeholder Neutral is passed).
pub fn build_card_sheet(variants: Vec<VariantRef>, tones: Vec<ToneRef>) -> Rc<StyleSheet> {
    let radius = || tokens().radius.lg();

    let mut sheet = StyleSheet::new(move |_vs: &VariantSet| StyleRules {
        gap: Some(tokens().spacing.sm()),
        border_top_left_radius: Some(radius()),
        border_top_right_radius: Some(radius()),
        border_bottom_left_radius: Some(radius()),
        border_bottom_right_radius: Some(radius()),
        border_top_width: Some(Tokenized::Literal(1.0)),
        border_right_width: Some(Tokenized::Literal(1.0)),
        border_bottom_width: Some(Tokenized::Literal(1.0)),
        border_left_width: Some(Tokenized::Literal(1.0)),
        border_top_color: Some(tokens().color.border()),
        border_right_color: Some(tokens().color.border()),
        border_bottom_color: Some(tokens().color.border()),
        border_left_color: Some(tokens().color.border()),
        background_transition: Some(Transition::new(250, Easing::EaseInOut)),
        color_transition: Some(Transition::new(250, Easing::EaseInOut)),
        border_top_color_transition: Some(Transition::new(250, Easing::EaseInOut)),
        ..Default::default()
    });

    for v in &variants {
        let v_c = v.clone();
        sheet = sheet.variant("variant", v.key(), move |_vs| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            let neutral = tones::Neutral;
            let ctx = ResolutionCtx {
                theme: theme_ref,
                tone: &neutral,
            };
            v_c.0.render(&ctx)
        });
    }

    sheet = sheet
        .variant("padding", "none", |_vs| StyleRules {
            padding_top: Some(Tokenized::Literal(Length::Px(0.0))),
            padding_bottom: Some(Tokenized::Literal(Length::Px(0.0))),
            padding_left: Some(Tokenized::Literal(Length::Px(0.0))),
            padding_right: Some(Tokenized::Literal(Length::Px(0.0))),
            ..Default::default()
        })
        .variant("padding", "sm", move |_vs| StyleRules {
            padding_top: Some(tokens().spacing.sm()),
            padding_bottom: Some(tokens().spacing.sm()),
            padding_left: Some(tokens().spacing.sm()),
            padding_right: Some(tokens().spacing.sm()),
            ..Default::default()
        })
        .variant("padding", "md", move |_vs| StyleRules {
            padding_top: Some(tokens().spacing.lg()),
            padding_bottom: Some(tokens().spacing.lg()),
            padding_left: Some(tokens().spacing.lg()),
            padding_right: Some(tokens().spacing.lg()),
            ..Default::default()
        })
        .variant("padding", "lg", move |_vs| StyleRules {
            padding_top: Some(tokens().spacing.xl()),
            padding_bottom: Some(tokens().spacing.xl()),
            padding_left: Some(tokens().spacing.xl()),
            padding_right: Some(tokens().spacing.xl()),
            ..Default::default()
        })
        .variant_default("variant", "flat")
        .variant_default("padding", "md");

    // NOTE: the intent tint deliberately does NOT live on a `tone` axis here,
    // even though every sibling sheet enumerates its tones. `StyleSheet`
    // stores axes in a `BTreeMap`, so per-axis arms merge in ALPHABETICAL axis
    // order — `"tone"` merges before `"variant"`, and Card's `variant` arms
    // set `background` (the Flat/Elevated surface), which would overwrite the
    // tint. The tint has to resolve after the surface, and the computed layer
    // is the only slot that does (base → axes → computed → overrides).
    //
    // The sheets that DO enumerate tones (Badge/Tag/Alert) dodge this by
    // folding both into ONE `appearance` axis keyed `{tone}_{variant}`, so
    // there's no cross-axis conflict to order. Card could follow suit — it's a
    // tones × variants arm expansion — but that's a behavioral restructure of
    // the public axis, not a mechanical conversion.
    //
    // Consequence: a TONED Card still resolves live. An untoned one (the
    // common case) premints, which is what the identity below unlocks.

    // Premint identity — without it the sheet has no premint class and every
    // Card falls through to the runtime engine.
    sheet.premint_as(&premint_identity("card", [variant_keys(&variants)]))
}

// Reactive-by-default: `#[props]` wraps each scalar-DATA field → `Reactive<…>`
// (`variant`/`padding`/`tone`). All three drive the Card surface style, so they
// route into the style sink reading `.get()` live; `children` is the children
// category and stays bare. A bare value stays a zero-cost `Static` snapshot (the
// fast path — keeps the build-time `StyleSource::Static`); a `Signal`/`rx!`
// re-styles in place.
#[runtime_core::props]
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct CardProps {
    /// Surface skeleton: built-in [`variant::Flat`] (page surface) or
    /// [`variant::Elevated`] (raised surface + shadow), or an
    /// app-installed custom variant. Default Flat.
    pub variant: VariantRef,
    /// Inner padding scale (None/Sm/Md/Lg → theme spacing tokens).
    /// Default Md.
    pub padding: CardPadding,
    /// Optional intent tint. When `Some`, the card paints a muted
    /// tone-tinted background and matching border (the same "Soft"
    /// treatment Alert uses) instead of the variant's surface color —
    /// for support/crisis/info panels that need to read as intent-colored.
    /// When `None` (the default), Flat/Elevated keep their surface look.
    pub tone: Option<ToneRef>,
    /// Style override for the card surface (background, border, radius, shadow,
    /// …), layered on top of the resolved variant/padding/tone style — the top
    /// resolution layer, so any field set here wins. See [`crate::slot_override`].
    ///
    /// This is also how you clip contents to the rounded corners: by default a
    /// card does NOT clip (content may extend past the radius — the friendlier
    /// default for overhanging popovers/menus), so pass an override that sets
    /// `overflow: Overflow::Hidden` for an edge-to-edge image or coloured header
    /// that should follow the corner curve. It clips on every backend (the same
    /// mechanism Modal uses for its rounded frame). iOS caveat: a clipping layer
    /// can't also cast the Elevated variant's drop shadow — pair the clip with
    /// `Flat`, or nest a clipped inner card in an unclipped elevated one.
    #[prop(static)]
    pub style: Option<Rc<StyleSheet>>,
    /// Card contents. Incoming fragments are flattened via
    /// `ChildList::append_to` before rendering inside the surface.
    pub children: Vec<Element>,
}

impl Default for CardProps {
    fn default() -> Self {
        Self {
            variant: variant::Flat.into(),
            padding: Reactive::Static(CardPadding::default()),
            tone: Reactive::Static(None),
            style: None,
            children: Vec::new(),
        }
    }
}

/// Surface container that wraps its children in a themed, bordered,
/// rounded panel. The `variant` picks the background/shadow treatment
/// and `padding` the inner spacing.
#[component(children)]
pub fn Card(props: CardProps) -> Element {
    // The style is REACTIVE when any style-driving prop is live; otherwise it's
    // the build-time fast path (one `StyleSource::Static`, no flicker — see
    // Button). The closure reads each prop's `.get()` INSIDE so the apply-style
    // Effect subscribes to whichever are dynamic.
    let style_is_reactive =
        !props.variant.is_static() || !props.padding.is_static() || !props.tone.is_static();

    let make_style = {
        let variant = props.variant.clone();
        let padding = props.padding.clone();
        let tone = props.tone.clone();
        let style_ovr = props.style.clone();
        move || -> StyleApplication {
            let variant_key = variant.get().key().to_string();
            let padding_key = padding.get().as_variant_str().to_string();
            let mut style = StyleApplication::new(card_sheet())
                .with("variant", variant_key)
                .with("padding", padding_key);

            // Intent tint — overlays the variant's surface bg/border with the
            // tone's Soft slots. Rides the INLINE layer, which resolves after
            // the `variant` axis (see `build_card_sheet`) — and, unlike the
            // old `with_computed` spelling, does not disqualify the card
            // from preminting (a `--premint-only` app panicked on any toned
            // Card). Tones are an OPEN set (author-extensible via the tone
            // macro), so an enumerated axis can't carry them; the slot
            // values are theme TOKENS, so the tint stays live across theme
            // swaps on every backend.
            if let Some(tone) = tone.get() {
                let theme_rc = active_theme();
                let theme_ref = theme_rc
                    .downcast_ref::<IdeaThemeRef>()
                    .expect("idea-ui: no IdeaTheme installed");
                let bg = tone.soft_bg(theme_ref);
                let border = tone.stroke_color(theme_ref);
                let fg = tone.soft_fg(theme_ref);
                style = style.with_inline(StyleRules {
                    background: Some(bg),
                    color: Some(fg),
                    border_top_color: Some(border.clone()),
                    border_right_color: Some(border.clone()),
                    border_bottom_color: Some(border.clone()),
                    border_left_color: Some(border),
                    ..Default::default()
                });
            }

            // Author surface override wins (top resolution layer). This is also
            // where a caller opts into corner clipping — `overflow: hidden` in
            // the override sheet clips children to the card's border radius.
            apply_override(style, &style_ovr)
        }
    };

    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    if style_is_reactive {
        ui! { view(style = make_style) { children } }
    } else {
        ui! { view(style = make_style()) { children } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::extensible::{tone, Tone};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::resolve_style;

    fn theme() {
        install_idea_theme(light_theme());
    }

    fn view_style(card: Element) -> StyleApplication {
        match classify(card) {
            P::View { style, .. } => match style.expect("Card view has a style") {
                TStyle::App(a) => a,
                _ => panic!("Card uses a static style source"),
            },
            _ => panic!("Card renders a view"),
        }
    }

    // D7: a toned Card paints the tone's Soft tint as its background,
    // distinct from the surface bg a tone-less Flat card renders.
    #[test]
    fn tone_tints_background_distinct_from_surface() {
        with_test_world(|| {
            theme();
            let toned = CardProps {
                tone: Reactive::Static(Some(tone::Danger.into())),
                ..Default::default()
            };
            let toned_bg = resolve_style(&view_style(Card(toned)))
                .background
                .clone()
                .expect("toned card sets a background");

            let plain = CardProps::default();
            let plain_bg = resolve_style(&view_style(Card(plain)))
                .background
                .clone()
                .expect("Flat card sets a surface background");

            assert_ne!(
                toned_bg, plain_bg,
                "a Danger-toned card must read differently from a plain surface card"
            );
            // The tint matches the Danger tone's Soft slot (the same tint
            // Alert's Soft variant uses).
            let theme_rc = active_theme();
            let expected =
                tone::Danger.soft_bg(theme_rc.downcast_ref::<IdeaThemeRef>().unwrap());
            assert_eq!(toned_bg, expected, "tint is the tone's soft_bg");
    });
    }

    // D7: with no tone, Flat/Elevated keep their surface look unchanged —
    // the tint layer is absent entirely.
    #[test]
    fn no_tone_keeps_surface_look() {
        with_test_world(|| {
            theme();
            let plain = CardProps::default();
            let app = view_style(Card(plain));
            assert!(
                app.inline().is_none(),
                "a tone-less Card attaches no tint layer"
            );
    });
    }

    // A toned Card must still PREMINT: the tint rides the INLINE layer
    // (out-of-band, applied over the preminted classes), never a
    // `with_computed` layer — a computed layer is a premint disqualifier,
    // so the old spelling made any `Card(tone = …)` panic at mount in a
    // `--premint-only` app. Fails against the computed spelling
    // (`preminted_class_list()` is `None` for a computed-carrying
    // application).
    #[test]
    fn regression_toned_card_premints_tint_via_inline_layer() {
        with_test_world(|| {
            theme();
            let toned = CardProps {
                tone: Reactive::Static(Some(tone::Danger.into())),
                ..Default::default()
            };
            let app = view_style(Card(toned));
            assert!(
                app.preminted_class_list().is_some(),
                "a toned Card must premint (tint rides the inline layer)"
            );
            let inline = app.inline().expect("tone tint rides the inline layer");
            assert!(inline.background.is_some(), "tint carries the Soft bg");
        });
    }

    // Clipping is a style attribute, not a bespoke prop: an `overflow: hidden`
    // in the `style` override clips children to the card's border radius, while
    // the default (no override) leaves overflow unset so content may overhang.
    #[test]
    fn style_override_overflow_clips_to_radius() {
        with_test_world(|| {
            theme();
            let clip = Rc::new(StyleSheet::r#static(StyleRules {
                overflow: Some(Overflow::Hidden),
                ..Default::default()
            }));
            let clipped = CardProps {
                style: Some(clip),
                ..Default::default()
            };
            assert_eq!(
                resolve_style(&view_style(Card(clipped))).overflow,
                Some(Overflow::Hidden),
                "overflow:hidden in the style override clips children to the radius",
            );

            let default = CardProps::default();
            assert_eq!(
                resolve_style(&view_style(Card(default))).overflow,
                None,
                "the default doesn't clip — content may extend past the radius",
            );
    });
    }

    // Slot override: the root `style` layers onto the card surface and wins
    // (background here) over the variant style — and can even turn clip back
    // off, since it's the top resolution layer.
    #[test]
    fn style_override_wins_over_variant() {
        with_test_world(|| {
            theme();
            let ovr = Rc::new(StyleSheet::r#static(StyleRules {
                background: Some(Tokenized::Literal(runtime_core::Color("#123456".into()))),
                ..Default::default()
            }));
            let props = CardProps {
                style: Some(ovr),
                ..Default::default()
            };
            assert_eq!(
                resolve_style(&view_style(Card(props)))
                    .background
                    .as_ref()
                    .map(|c| c.resolve().0.to_ascii_lowercase()),
                Some("#123456".to_string()),
                "style override sets the card background over the variant surface",
            );
    });
    }
}
