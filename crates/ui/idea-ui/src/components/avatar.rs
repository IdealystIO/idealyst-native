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
//! Styling comes from the installed Avatar sheets
//! (`idea_theme::extensible::AvatarSheetBuilder`); apps with custom tones
//! install an extended pair via
//! `install_avatar_sheets(AvatarSheetBuilder::new().add_tone(..).build())`.

use std::rc::Rc;

use runtime_core::{
    component, image_from, ui, Element, IdealystSchema, ImageSource, IntoElement, IntoStyleProp,
    Reactive, StyleApplication, StyleProp, StyleSheet,
};

use idea_theme::extensible::{installed_avatar_sheets, tone, variant, ToneRef, VariantRef};

use crate::stylesheets::AvatarImage;

/// Avatar diameter scale. The string keys match the `size` arms the
/// idea-theme `AvatarSheetBuilder` generates.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema, runtime_core::Remote)]
pub enum AvatarSize {
    /// 24px.
    Xs,
    /// 32px.
    Sm,
    /// 40px (default).
    #[default]
    Md,
    /// 56px.
    Lg,
    /// 80px.
    Xl,
}

impl AvatarSize {
    /// The `size` variant-axis key for this scale.
    pub fn as_variant_str(self) -> &'static str {
        match self {
            AvatarSize::Xs => "xs",
            AvatarSize::Sm => "sm",
            AvatarSize::Md => "md",
            AvatarSize::Lg => "lg",
            AvatarSize::Xl => "xl",
        }
    }
}

// `VariantEnum` lets the docs-app `DocControls` derive render a size
// picker; the inherent `as_variant_str` above is what the component calls.
impl runtime_core::VariantEnum for AvatarSize {
    fn as_variant_str(self) -> &'static str {
        AvatarSize::as_variant_str(self)
    }
    fn all_variants() -> &'static [Self] {
        &[AvatarSize::Xs, AvatarSize::Sm, AvatarSize::Md, AvatarSize::Lg, AvatarSize::Xl]
    }
}

// Reactive-by-default: `#[props]` wraps `tone`/`variant`/`size` →
// `Reactive<…>` (and would wrap `src`, but it's `Option<ImageSource>` — see
// below); `initials` is already reactive. The style-driving props route into
// the container/text style sinks, read `.get()` INSIDE so the apply-style
// Effect subscribes to whichever are live. `src` selects WHICH subtree
// renders (image vs initials) — a structural branch.
#[runtime_core::props]
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
    pub variant: VariantRef,
    /// Diameter scale (Xs/Sm/Md/Lg/Xl).
    pub size: AvatarSize,
}

impl Default for AvatarProps {
    fn default() -> Self {
        Self {
            src: Reactive::Static(None),
            initials: Reactive::Static(String::new()),
            tone: tone::Neutral.into(),
            variant: variant::Soft.into(),
            size: Reactive::Static(AvatarSize::default()),
        }
    }
}

/// Circular user-identity element. Renders the `src` image when set,
/// otherwise the `initials` on a `tone`-tinted disc.
#[component]
pub fn Avatar(props: &AvatarProps) -> Element {
    // The disc and the initials apply the SAME `appearance` + `size` keys,
    // each on its own sheet: native text doesn't inherit color from the
    // disc, so the initials' sheet carries the appearance's foreground
    // itself.
    //
    // When any style prop is live the style is a closure that reads `.get()`
    // INSIDE, so the apply-style Effect subscribes to it. When all are static
    // it is a plain `StyleApplication`, resolved once at build: no per-node
    // Effect, and — unlike a closure, which always takes the live engine —
    // it can stamp the sheet's preminted class.
    let sheets = installed_avatar_sheets();
    let style_is_reactive =
        !props.tone.is_static() || !props.variant.is_static() || !props.size.is_static();
    let make_app = {
        let tone = props.tone.clone();
        let variant = props.variant.clone();
        let size = props.size.clone();
        move |sheet: &Rc<StyleSheet>| -> StyleApplication {
            StyleApplication::new(sheet.clone())
                .with("appearance", format!("{}_{}", tone.get().key(), variant.get().key()))
                .with("size", size.get().as_variant_str())
        }
    };
    let style_for = move |sheet: Rc<StyleSheet>| {
        let make_app = make_app.clone();
        move || -> StyleProp {
            if style_is_reactive {
                let make_app = make_app.clone();
                let sheet = sheet.clone();
                (move || make_app(&sheet)).into_style_prop()
            } else {
                make_app(&sheet).into_style_prop()
            }
        }
    };
    let container_style = style_for(sheets.container.clone());
    let text_style = style_for(sheets.text.clone());

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
            let img = image_from(source)
                .with_style(StyleApplication::new(AvatarImage::sheet()))
                .into_element();
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
            // static application — covers both style paths the old match did.
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
