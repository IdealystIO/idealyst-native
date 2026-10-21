//! `Avatar` — circular user-identity element.
//!
//! Renders an image when `src` is set, otherwise falls back to
//! `initials` on a tinted disc. The disc's color is a [`ToneRef`] —
//! usually a palette hue (`tone::Violet`, …), since an avatar is a
//! person/object placeholder rather than a semantic action, but the
//! semantic tones work too. `variant` picks the treatment: `Soft`
//! (default — tinted disc, hue-colored initials) or `Filled` (solid
//! disc, contrasting initials).
//!
//! ```ignore
//! ui! {
//!     Avatar(
//!         initials = "AB".to_string(),
//!         // A stable color per user, from any string id.
//!         tone = tone::palette_from_seed(&user.id),
//!         size = AvatarSize::Md,
//!     )
//! }
//! ```
//!
//! Styling is [`AvatarStyles`]: `root` (the disc), `initials` and
//! `image` (a photo filling the disc). The disc and the initials draw one
//! compound per `(tone, variant)` over the categorization tones; the
//! initials carry the fill's foreground on their own node (native text
//! inherits no color from the disc).

use runtime_core::{
    component, component_styles, image_from, ui, AlignItems, AlignSelf, Element, FontWeight, IdealystSchema,
    ImageSource, IntoElement, JustifyContent, Length, ObjectFit, Overflow, Reactive, SlotRestyle, TextAlign,
    TextTransform,
};

use idea_theme::extensible::{categorization_tones, tone, ToneRef};

use crate::components::tone_slots::{foreground_arms, surface_arms, with_tone};
use crate::modifiers::{marker_into_axis, variant};
use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Avatar`].
    pub Avatar<IdeaThemeRef> {
        /// The treatment — a tone-aware recipe per value.
        axis variant { filled, #[default] soft, outlined, ghost }

        /// The disc: fill, clip, diameter.
        slot root {
            extend(root_surfaces)
            base(t) {
                border_radius: t.radius.pill(),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                // Hug: a row of mixed-size avatars centers rather than
                // top-aligning under the parent's `align-items: stretch`.
                align_self: AlignSelf::Center,
                overflow: Overflow::Hidden,
            }
            /// The diameter: 24 / 32 / 40 / 56 / 80 px.
            variant size {
                xs(_t) { width: 24.0, height: 24.0 }
                sm(_t) { width: 32.0, height: 32.0 }
                #[default]
                md(_t) { width: 40.0, height: 40.0 }
                lg(_t) { width: 56.0, height: 56.0 }
                xl(_t) { width: 80.0, height: 80.0 }
            }
            transitions {
                background: 250ms EaseInOut,
                color: 250ms EaseInOut,
            }
        }

        /// The initials, in the fill's foreground. The line height is the
        /// diameter, so the text centers in the disc on every backend
        /// without relying on flex centering of a text node.
        slot initials {
            extend(initials_foregrounds)
            base(_t) {
                font_weight: FontWeight::SemiBold,
                text_align: TextAlign::Center,
                letter_spacing: 0.5,
                text_transform: TextTransform::Uppercase,
            }
            variant size {
                xs(_t) { font_size: Length::Px(10.0), line_height: 24.0 }
                sm(t) { font_size: t.typography.body_sm_size(), line_height: 32.0 }
                #[default]
                md(t) { font_size: t.typography.body_size(), line_height: 40.0 }
                lg(_t) { font_size: Length::Px(20.0), line_height: 56.0 }
                xl(_t) { font_size: Length::Px(28.0), line_height: 80.0 }
            }
        }

        /// The photo inside the disc. Fill + `Cover`: the disc is a
        /// fixed-size clipped circle, so an UNSIZED image renders at its
        /// natural size and the clip shows only its center. Cover keeps the
        /// disc filled for any source aspect without distortion.
        slot image {
            base(_t) {
                width: Length::pct(100.0),
                height: Length::pct(100.0),
                object_fit: ObjectFit::Cover,
            }
        }
    }
}

fn root_surfaces(s: &mut SlotRestyle<'_, AvatarStyles>) {
    surface_arms(s, &categorization_tones());
}

fn initials_foregrounds(s: &mut SlotRestyle<'_, AvatarStyles>) {
    foreground_arms(s, &categorization_tones());
}

marker_into_axis!(AvatarVariant {
    variant::Filled => Filled,
    variant::Soft => Soft,
    variant::Outlined => Outlined,
    variant::Ghost => Ghost,
});

// Reactive-by-default: `#[props]` wraps `tone`/`variant`/`size` →
// `Reactive<…>` (and would wrap `src`, but it's `Option<ImageSource>` — see
// below); `initials` is already reactive. The style-driving props route into
// the container/text style sinks, read `.get()` INSIDE so the apply-style
// Effect subscribes to whichever are live. `src` selects WHICH subtree
// renders (image vs initials) — a structural branch.
#[runtime_core::props(styles = AvatarStyles)]
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct AvatarProps {
    /// Optional image source — a URL **or** a bundled
    /// [`Asset`](runtime_core::assets::Asset), via
    /// [`ImageSource`]. When `Some`, an image renders and the initials are
    /// hidden; when `None`, the initials show. Build it from a string
    /// (`Some("https://…".into())`) or an asset (`Some(LOGO.into())`).
    #[schema(constraint = "ImageSource — URL string or bundled Asset")]
    pub src: Option<ImageSource>,
    /// Fallback text rendered when `src` is `None`.
    /// `Reactive<String>` — static or live (signal/`rx!`).
    pub initials: Reactive<String>,
    /// Disc color. A palette hue (`tone::Blue`, `tone::Violet`, … or
    /// `tone::palette_from_seed(id)` for a stable per-user hue) or a
    /// semantic tone. Default Neutral.
    pub tone: ToneRef,
    /// Treatment: Soft (tinted disc, hue-colored initials — default) or
    /// Filled (solid disc, contrasting initials); Outlined / Ghost also
    /// apply.
    pub variant: AvatarVariant,
    /// Diameter scale (Xs/Sm/Md/Lg/Xl, or an app-defined size).
    pub size: AvatarSize,
}

impl Default for AvatarProps {
    fn default() -> Self {
        Self {
            src: Reactive::Static(None),
            initials: Reactive::Static(String::new()),
            tone: tone::Neutral.into(),
            variant: Reactive::Static(AvatarVariant::default()),
            size: Reactive::Static(AvatarSize::default()),
            styles: Default::default(),
        }
    }
}

/// Circular user-identity element. Renders the `src` image when set,
/// otherwise the `initials` on a `tone`-tinted disc.
#[component]
pub fn Avatar(props: &AvatarProps) -> Element {
    // The disc and the initials apply the same tone/variant/size, each on
    // its own slot: native text doesn't inherit color from the disc.
    let styles = props.styles.clone();
    let values =
        with_tone(AvatarStyles::values().set(props.variant.clone()).set(props.size.clone()), &props.tone);
    let container_style = {
        let values = values.clone();
        let styles = styles.clone();
        move || AvatarStyles::root().values(&values).overridden(styles.root.as_ref())
    };
    let text_style = {
        let values = values.clone();
        let styles = styles.clone();
        move || AvatarStyles::initials().values(&values).overridden(styles.initials.as_ref())
    };
    let image_style = {
        let styles = styles.clone();
        move || AvatarStyles::image().values(&values).overridden(styles.image.as_ref())
    };

    let initials = props.initials.clone();

    // `src` selects WHICH subtree renders (image vs initials) — a structural
    // branch. Routed via `when(|| src.get().is_some(), …)` so a live `src`
    // flipping between `Some`/`None` swaps the image↔initials subtree in place
    // (the active branch is rebuilt, the hidden one's effects dropped). When
    // `src` is `Static` we keep the build-time branch (no per-node `When`
    // anchor) — mirrors how the style sinks above collapse for static props.
    let build_image = {
        let src = props.src.clone();
        let container_style = container_style.clone();
        let image_style = image_style.clone();
        move || {
            // Read `src` INSIDE so the `when` Effect subscribes; the source is
            // `Some` here because the `cond` selected this branch.
            let source = src.get().expect("when(src.is_some) image branch");
            // Sized to FILL the disc (see `AvatarImage`): unstyled, the
            // image renders at natural size and the clipped container
            // shows only its center.
            //
            // Built with `image_from` rather than `ui! { image(..) }`: the
            // macro's `image` takes a `src` string OR an `asset`, never an
            // `ImageSource`, so expressing it in `ui!` means re-matching
            // `ImageSource`'s variants here — the dispatch `image_from` owns
            // (including its loud arm for a variant added upstream).
            // idealyst-lint-disable-next-line prefer-ui-macro -- `ui!` `image` has no `ImageSource` prop; `image_from` owns the variant dispatch
            let img = image_from(source).with_style(image_style()).into_element();
            ui! {
                view(style = container_style()) {
                    img
                }
            }
        }
    };
    let build_initials = {
        let container_style = container_style.clone();
        let text_style = text_style.clone();
        let initials = initials.clone();
        move || {
            let initials = initials.clone();
            ui! {
                view(style = container_style()) {
                    text(style = text_style()) { initials }
                }
            }
        }
    };

    if props.src.is_static() {
        // Static fast path: resolve the branch once, no `When` anchor.
        if props.src.get().is_some() {
            build_image()
        } else {
            build_initials()
        }
    } else {
        let src = props.src.clone();
        // idealyst-lint-disable-next-line prefer-ui-control-flow -- static-prop fast path (see `components/mod.rs`)
        runtime_core::when(move || src.get().is_some(), build_image, build_initials)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, TStyle, P};
    use idea_theme::testing::{commit, with_test_world};
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, signal, AlignSelf, Color, Tokenized};

    /// The disc's and the initials' styles of an initials Avatar.
    fn disc_and_text(props: &AvatarProps) -> (TStyle, TStyle) {
        match classify(Avatar(props)) {
            P::View { mut children, style, .. } => (
                style.expect("disc is styled"),
                classify(children.remove(0)).style().expect("initials are styled"),
            ),
            _ => panic!("Avatar renders a styled View"),
        }
    }

    fn token_name(t: &Option<Tokenized<Color>>) -> Option<&'static str> {
        t.as_ref().and_then(|t| t.name())
    }

    // Regression: the initials took their color only from the disc's
    // `color`, which native `UILabel`/`TextView` never inherit (only web's
    // cascade does), so a Filled avatar showed default-colored initials on
    // its fill off the web. The text node must carry the contrast color
    // itself — the same slot pair the disc paints.
    #[test]
    fn regression_avatar_initials_carry_contrast_color_on_the_text_node() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let (disc, text) = disc_and_text(&AvatarProps {
                initials: "AB".into(),
                tone: tone::Teal.into(),
                variant: variant::Filled.into(),
                ..Default::default()
            });
            assert_eq!(
                token_name(&resolve_style(&disc.application()).background),
                Some("palette-teal-solid-bg")
            );
            assert_eq!(
                token_name(&resolve_style(&text.application()).color),
                Some("palette-teal-solid-text"),
                "the initials must carry the fill's contrast color on their own node"
            );
        });
    }

    /// Default Avatar: a neutral soft disc, with the matching soft text.
    #[test]
    fn avatar_defaults_to_neutral_soft() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let (disc, text) =
                disc_and_text(&AvatarProps { initials: "AB".into(), ..Default::default() });
            assert_eq!(
                token_name(&resolve_style(&disc.application()).background),
                Some("intent-neutral-soft-bg")
            );
            assert_eq!(
                token_name(&resolve_style(&text.application()).color),
                Some("intent-neutral-soft-text")
            );
        });
    }

    /// All-static props attach a plain `StyleApplication` — resolvable once
    /// and eligible for the sheet's preminted class — not a closure, which
    /// always takes the live engine and costs a per-node Effect.
    #[test]
    fn static_props_attach_a_static_style() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let (disc, text) = disc_and_text(&AvatarProps {
                initials: "AB".into(),
                tone: tone::Blue.into(),
                ..Default::default()
            });
            assert!(matches!(disc, TStyle::App(_)), "static disc style must not be a closure");
            assert!(matches!(text, TStyle::App(_)), "static text style must not be a closure");
        });
    }

    /// A live `tone` re-styles the disc and the initials in place.
    #[test]
    fn live_tone_restyles_disc_and_initials() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let hue = signal(ToneRef::from(tone::Red));
            let (disc, text) = disc_and_text(&AvatarProps {
                initials: "AB".into(),
                tone: hue.read_only().into(),
                variant: variant::Filled.into(),
                ..Default::default()
            });
            assert!(matches!(disc, TStyle::AppFn(_)), "a live tone needs a reactive style");
            assert_eq!(
                token_name(&resolve_style(&text.application()).color),
                Some("palette-red-solid-text")
            );
            hue.set(tone::Violet.into());
            commit();
            assert_eq!(
                token_name(&resolve_style(&disc.application()).background),
                Some("palette-violet-solid-bg")
            );
            assert_eq!(
                token_name(&resolve_style(&text.application()).color),
                Some("palette-violet-solid-text")
            );
        });
    }

    // Regression: an Avatar is a fixed-size atom — it must center on its
    // parent's cross axis (`align_self: Center`) so a row of mixed-size
    // avatars centers instead of top-aligning under the default
    // `align-items: stretch` (the Avatar "Sizes" row report).
    #[test]
    fn avatar_centers_on_cross_axis() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            // `application()` evaluates a reactive style closure or returns the
            // static application — covers both style paths.
            let app = match classify(Avatar(&AvatarProps { initials: "AB".into(), ..Default::default() })) {
                P::View { style, .. } => style.expect("Avatar attaches a style").application(),
                _ => panic!("Avatar renders a styled View"),
            };
            assert_eq!(
                resolve_style(&app).align_self,
                Some(AlignSelf::Center),
                "an Avatar centers on the cross axis instead of stretching/top-aligning"
            );
    });
    }
}
