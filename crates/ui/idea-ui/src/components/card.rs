//! `Card` — surface container.
//!
//! ```ignore
//! ui! {
//!     Card(variant = card::variant::Elevated, padding = CardPadding::Md) {
//!         Typography(content = "Stats", kind = typography_kind::H2)
//!     }
//! }
//! ```
//!
//! Four built-in variants ([`CardVariant`]):
//!
//! - `Regular` (default) — page-surface fill + 1px border.
//! - `Outline` — the border alone, no fill. The base for a dashed or
//!   dotted frame: override `border_style` and nothing else competes with
//!   the stroke.
//! - `Flat` — secondary (`surface_alt`) fill, no border.
//! - `Elevated` — surface-alt fill + drop shadow.
//!
//! They read the theme's surface colors directly — no intent palette.
//! Styling is [`CardStyles`]' `root` slot; an app restyles a variant, or
//! adds one, at boot:
//!
//! ```ignore
//! const GLASS: CardVariant = CardVariant::define("glass");
//! CardStyles::restyle(|s| {
//!     s.root().arm(CardVariant::Regular, |t| StyleRules { shadow: …, ..Default::default() });
//!     s.root().arm(GLASS, |t| StyleRules { background: Some(t.color.surface_alt()), … });
//! });
//! ```
//!
//! A dashed drop-zone frame is one instance's override:
//!
//! ```ignore
//! Card(variant = card::variant::Outline, styles = CardSlotStyles::default().root(dashed)) { … }
//! ```

use runtime_core::{
    component, component_styles, ui, ChildList, Element, IdealystSchema, Length, Reactive, StyleRules,
};

use idea_theme::extensible::ToneRef;
use idea_theme::theme::IdeaThemeRef;

use crate::modifiers::marker_into_axis;

component_styles! {
    /// The styled sections of [`Card`].
    pub Card<IdeaThemeRef> {
        /// The surface.
        slot root {
            base(t) {
                gap: t.spacing.sm(),
                border_radius: t.radius.lg(),
                border_width: 1.0,
                border_color: t.color.border(),
            }
            /// The surface treatment. Every arm sets its fill (and Flat its
            /// border widths) explicitly rather than leaving them unset: a
            /// card whose variant switches live must CLEAR the previous fill,
            /// and a native backend leaves an unset property as it was.
            variant variant {
                #[default]
                regular(t) { background: t.color.surface() }
                outline(_t) { background: runtime_core::Color("transparent".into()) }
                flat(t) {
                    background: t.color.surface_alt(),
                    border_width: 0.0,
                }
                elevated(t) {
                    background: t.color.surface_alt(),
                    shadow: runtime_core::Shadow {
                        x: 0.0,
                        y: 4.0,
                        blur: 16.0,
                        color: t.effect.shadow(),
                    },
                }
            }
            /// Inner padding: none / the theme's sm / lg / xl spacing.
            variant padding {
                none(_t) { padding: Length::Px(0.0) }
                sm(t) { padding: t.spacing.sm() }
                #[default]
                md(t) { padding: t.spacing.lg() }
                lg(t) { padding: t.spacing.xl() }
            }
            transitions {
                background: 250ms EaseInOut,
                color: 250ms EaseInOut,
                border_color: 250ms EaseInOut,
            }
        }
    }
}

/// The built-in Card variant markers: `variant = card::variant::Flat`
/// converts into [`CardVariant`] — and only into it, so a Card variant on
/// a Button does not compile.
///
/// ```compile_fail
/// let _: idea_ui::ButtonVariant = idea_ui::components::card::variant::Flat.into();
/// ```
pub mod variant {
    use runtime_core::IdealystSchema;

    /// Regular — page-surface background framed by the 1px border. The
    /// default.
    #[derive(Copy, Clone, Default, Debug, IdealystSchema)]
    #[schema(value_of = "CardVariant", via = "idea_ui::components::{card::variant}")]
    pub struct Regular;

    /// Outline — the border with no fill, so whatever is behind the card
    /// shows through.
    #[derive(Copy, Clone, Default, Debug, IdealystSchema)]
    #[schema(value_of = "CardVariant", via = "idea_ui::components::{card::variant}")]
    pub struct Outline;

    /// Flat — the secondary (`surface_alt`) fill with NO border: a panel
    /// set apart by its tone alone.
    #[derive(Copy, Clone, Default, Debug, IdealystSchema)]
    #[schema(value_of = "CardVariant", via = "idea_ui::components::{card::variant}")]
    pub struct Flat;

    /// Elevated — raised surface with a soft drop shadow (the theme's
    /// `effect-shadow` token). `surface_alt`, so the card reads as a layer
    /// above the page even where shadows don't render.
    #[derive(Copy, Clone, Default, Debug, IdealystSchema)]
    #[schema(value_of = "CardVariant", via = "idea_ui::components::{card::variant}")]
    pub struct Elevated;
}

marker_into_axis!(CardVariant {
    variant::Regular => Regular,
    variant::Outline => Outline,
    variant::Flat => Flat,
    variant::Elevated => Elevated,
});

// Reactive-by-default: `#[props]` wraps each scalar-DATA field → `Reactive<…>`
// (`variant`/`padding`/`tone`). All three drive the Card surface style, so they
// route into the style sink; `children` is the children category and stays
// bare. `styles` (from `styles = CardStyles`) is the per-instance override.
#[runtime_core::props(styles = CardStyles)]
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct CardProps {
    /// Surface skeleton: `Regular` (page surface + border), `Outline`
    /// (border only), `Flat` (secondary fill, no border), `Elevated`
    /// (raised surface + shadow), or an app-defined [`CardVariant`].
    /// Default Regular.
    pub variant: CardVariant,
    /// Inner padding scale (None/Sm/Md/Lg → theme spacing tokens).
    /// Default Md.
    pub padding: CardPadding,
    /// Optional intent tint. When `Some`, the card paints a muted
    /// tone-tinted background and matching border (the same "Soft"
    /// treatment Alert uses) instead of the variant's surface color —
    /// for support/crisis/info panels that need to read as intent-colored.
    /// When `None` (the default), the variant keeps its surface look.
    pub tone: Option<ToneRef>,
    /// Card contents. Incoming fragments are flattened via
    /// `ChildList::append_to` before rendering inside the surface.
    pub children: Vec<Element>,
}

impl Default for CardProps {
    fn default() -> Self {
        Self {
            variant: Reactive::Static(CardVariant::default()),
            padding: Reactive::Static(CardPadding::default()),
            tone: Reactive::Static(None),
            children: Vec::new(),
            styles: Default::default(),
        }
    }
}

/// The intent tint over the variant's surface: the tone's Soft background,
/// foreground and stroke. Token references, so it follows a theme swap.
fn tint(tone: Option<ToneRef>) -> StyleRules {
    let Some(tone) = tone else { return StyleRules::default() };
    idea_theme::extensible::with_idea_theme(|theme| {
        let border = tone.stroke_color(theme);
        StyleRules {
            background: Some(tone.soft_bg(theme)),
            color: Some(tone.soft_fg(theme)),
            border_top_color: Some(border.clone()),
            border_right_color: Some(border.clone()),
            border_bottom_color: Some(border.clone()),
            border_left_color: Some(border),
            ..Default::default()
        }
    })
}

/// Surface container that wraps its children in a themed, bordered,
/// rounded panel. The `variant` picks the background/shadow treatment
/// and `padding` the inner spacing.
///
/// The per-instance `styles.root` override is also how a card clips its
/// contents to the rounded corners: by default it does NOT clip (content
/// may extend past the radius — the friendlier default for overhanging
/// popovers/menus), so pass `overflow: Overflow::Hidden` for an
/// edge-to-edge image or coloured header that should follow the corner
/// curve. It clips on every backend, and `Elevated` still casts its drop
/// shadow when clipped — the Apple backends synthesize a separate shadow
/// layer for that combination, since one CALayer can't both clip and
/// cast. That works for an opaque surface, which every built-in variant
/// has; a *translucent* surface plus `Overflow::Hidden` renders without a
/// shadow on iOS/macOS.
#[component(children)]
pub fn Card(props: CardProps) -> Element {
    let style = CardStyles::root().set(props.variant.clone()).set(props.padding.clone());
    // The intent tint resolves after the `variant` axis (whose arms set the
    // surface background), so it rides the INLINE layer — and premints.
    // Tones are an open set (app-registered), and the values are theme
    // tokens, so the tint stays live across theme swaps on every backend.
    let style = match props.tone.clone() {
        Reactive::Static(None) => style,
        Reactive::Static(t) => style.inline(tint(t)),
        live => style.inline_with(move || tint(live.get())),
    };
    let style = style.overridden(props.styles.root.as_ref());

    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { view(style = style) { children } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::active_theme;
    use idea_theme::extensible::{tone, Tone};
    use runtime_core::{Overflow, StyleApplication, Tokenized};
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
    // distinct from the surface bg a tone-less Regular card renders.
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
                .expect("Regular card sets a surface background");

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

    // D7: with no tone, every variant keeps its surface look unchanged —
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
    // so it would make any `Card(tone = …)` panic at mount in a
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
            let clip = StyleRules {
                overflow: Some(Overflow::Hidden),
                ..Default::default()
            };
            let clipped = CardProps {
                styles: CardSlotStyles::default().root(clip),
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
            let ovr = StyleRules {
                background: Some(Tokenized::Literal(runtime_core::Color("#123456".into()))),
                ..Default::default()
            };
            let props = CardProps {
                styles: CardSlotStyles::default().root(ovr),
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

    fn resolved(variant: CardVariant, style: Option<StyleRules>) -> StyleRules {
        let styles = match style {
            Some(s) => CardSlotStyles::default().root(s),
            None => CardSlotStyles::default(),
        };
        let props = CardProps { variant: Reactive::Static(variant), styles, ..Default::default() };
        resolve_style(&view_style(Card(props))).as_ref().clone()
    }

    fn widths(r: &StyleRules) -> [Option<f32>; 4] {
        [&r.border_top_width, &r.border_right_width, &r.border_bottom_width, &r.border_left_width]
            .map(|w| w.as_ref().map(|t| t.resolve()))
    }

    fn surface(pick: fn(&idea_theme::theme::Colors) -> &Tokenized<runtime_core::Color>)
        -> Tokenized<runtime_core::Color>
    {
        use idea_theme::theme::IdeaTheme;
        let theme_rc = active_theme();
        let theme = theme_rc.downcast_ref::<IdeaThemeRef>().unwrap();
        pick(theme.colors()).clone()
    }

    // The default is Regular: the page surface inside the 1px frame — the
    // look every unannotated `Card(...)` had before the variants split.
    #[test]
    fn default_card_is_regular_surface_with_border() {
        with_test_world(|| {
            theme();
            let r = resolve_style(&view_style(Card(CardProps::default()))).as_ref().clone();
            assert_eq!(r.background, Some(surface(|c| &c.surface)));
            assert_eq!(widths(&r), [Some(1.0); 4]);
        });
    }

    // Outline is the frame alone. The fill is an explicit transparent
    // (not unset) so a live Regular → Outline switch clears it.
    #[test]
    fn outline_has_border_and_transparent_fill() {
        with_test_world(|| {
            theme();
            let r = resolved(variant::Outline.into(), None);
            assert_eq!(widths(&r), [Some(1.0); 4]);
            let bg = r.background.as_ref().map(|c| c.resolve().0);
            assert_eq!(bg.as_deref(), Some("transparent"));
        });
    }

    // Outline is the intended base for a dashed frame: an override that
    // sets only `border_style` keeps the variant's border and gains the
    // pattern.
    #[test]
    fn outline_takes_a_dashed_border_from_a_style_override() {
        with_test_world(|| {
            theme();
            let dashed = StyleRules {
                border_style: Some(runtime_core::BorderStyle::Dashed),
                ..Default::default()
            };
            let r = resolved(variant::Outline.into(), Some(dashed));
            assert_eq!(r.border_style, Some(runtime_core::BorderStyle::Dashed));
            assert_eq!(widths(&r), [Some(1.0); 4], "the frame survives the override");
        });
    }

    // Flat is the secondary fill with the border explicitly zeroed.
    #[test]
    fn flat_is_secondary_fill_without_border() {
        with_test_world(|| {
            theme();
            let r = resolved(variant::Flat.into(), None);
            assert_eq!(r.background, Some(surface(|c| &c.surface_alt)));
            assert_eq!(widths(&r), [Some(0.0); 4]);
        });
    }

    // Each built-in variant must be a real arm on the default sheet — an
    // unregistered key resolves to the base (no fill at all).
    #[test]
    fn every_builtin_variant_resolves_distinctly() {
        with_test_world(|| {
            theme();
            let all: [CardVariant; 4] =
                [CardVariant::Regular, CardVariant::Outline, CardVariant::Flat, CardVariant::Elevated];
            let keys: Vec<String> =
                all.iter().map(|v| resolved(v.clone(), None).content_key()).collect();
            for i in 0..keys.len() {
                for j in (i + 1)..keys.len() {
                    assert_ne!(keys[i], keys[j], "variants {i} and {j} resolve the same");
                }
            }
        });
    }

    fn reset_styles() {
        runtime_core::component_styles::__reset::<CardStyles>();
    }

    fn lifted() -> StyleRules {
        StyleRules {
            shadow: Some(runtime_core::Shadow {
                x: 0.0,
                y: 2.0,
                blur: 8.0,
                color: runtime_core::Color("rgba(0,0,0,0.2)".into()).into(),
            }),
            ..Default::default()
        }
    }

    /// Restyling one built-in variant changes that arm and nothing else: the
    /// other variants keep theirs (an app that replaced the whole sheet to
    /// change one variant used to lose every other built-in), and the
    /// restyled arm keeps the properties the restyle leaves unset.
    #[test]
    fn restyling_one_variant_keeps_the_others_and_merges() {
        with_test_world(|| {
            reset_styles();
            theme();
            CardStyles::restyle(|s| {
                s.root().arm(CardVariant::Regular, |_| lifted());
            });
            let flat = resolved(CardVariant::Flat, None);
            assert_eq!(flat.background, Some(surface(|c| &c.surface_alt)));
            assert_eq!(widths(&flat), [Some(0.0); 4], "Flat keeps its borderless arm");
            let regular = resolved(CardVariant::Regular, None);
            assert!(regular.shadow.is_some(), "the app's shadow lands on `regular`");
            assert_eq!(regular.background, Some(surface(|c| &c.surface)), "and the surface stays");
            reset_styles();
        });
    }

    /// An app-defined variant draws through its restyle arm; one nothing
    /// styles warns once in debug builds instead of silently drawing the
    /// base.
    #[test]
    fn a_custom_variant_draws_and_an_unstyled_one_warns() {
        const GLASS: CardVariant = CardVariant::define("glass");
        const FROSTED: CardVariant = CardVariant::define("frosted_unstyled");
        with_test_world(|| {
            reset_styles();
            theme();
            CardStyles::restyle(|s| {
                s.root().arm(GLASS, |t| StyleRules { background: Some(t.color.surface_alt()), ..lifted() });
            });
            let glass = resolved(GLASS, None);
            assert!(glass.shadow.is_some());
            let before = runtime_core::component_styles::__warn_count();
            let _ = resolved(FROSTED, None);
            #[cfg(debug_assertions)]
            assert_eq!(runtime_core::component_styles::__warn_count(), before + 1, "an unstyled key warns");
            let _ = resolved(FROSTED, None);
            #[cfg(debug_assertions)]
            assert_eq!(runtime_core::component_styles::__warn_count(), before + 1, "once");
            reset_styles();
        });
    }
}