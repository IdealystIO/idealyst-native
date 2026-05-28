//! Programmatic stylesheets driven by the modifier trait surface.
//!
//! This is the architecturally-correct path: instead of components
//! computing their styles at apply time via a "computed layer", we
//! build a `StyleSheet` at app startup that has one variant arm per
//! `(tone, variant)` combination. The framework's existing pregen
//! mechanism then resolves every arm into a CSS class once, and
//! apply-style is a className lookup. No FOUC, no dynamic CSS mint.
//!
//! **Built-in vs custom modifiers.** A builder starts pre-loaded with
//! the built-in modifier ZSTs (Primary tone, Filled variant, Md size,
//! Md shape, etc.). Apps that add custom modifiers — `Hype` tone,
//! `Elevated` variant — append them via `.add_tone(...)` / `.add_variant(...)`
//! before building. The Cartesian product of all registered modifiers
//! ends up as stylesheet arms, all pre-generated together.
//!
//! **Per-component sheets.** Each component (Button, Typography, etc.)
//! has its own sheet, installed once at app startup. Apps that don't
//! need custom modifiers can install the default sheet via the
//! convenience function on each component module.

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{
    Easing, FontWeight, StyleRules, StyleSheet, TextAlign, Tokenized, Transition, VariantSet,
};

use crate::theme::{IdeaTheme, IdeaThemeRef};
use crate::theme_runtime::active_theme;

use super::{
    ButtonSizeRef, RefBuiltins, ResolutionCtx, ShapeRef, ToneRef, TypographyKindRef, VariantRef,
};

// =============================================================================
// Thread-local sheet stashes — one per component
// =============================================================================
//
// `RefCell<Option<Rc<StyleSheet>>>` per component. App startup calls
// `install_<component>_sheet(sheet)`. Components read via
// `installed_<component>_sheet()`. Re-installation replaces the prior
// sheet (supports hot-reload + per-app overrides).

thread_local! {
    static BUTTON_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
}

/// Install a Button stylesheet for the current thread. The next
/// `installed_button_sheet()` call returns this sheet. Apps call this
/// once at startup; `install_idea_theme` calls it automatically with
/// the default builder output, so apps that just want built-ins don't
/// have to touch it.
pub fn install_button_sheet(sheet: Rc<StyleSheet>) {
    BUTTON_SHEET.with(|s| *s.borrow_mut() = Some(sheet));
}

/// The currently-installed Button stylesheet. Panics if no sheet has
/// been installed — call [`install_button_sheet`] (or
/// `install_idea_theme`, which calls it internally) before mounting.
pub fn installed_button_sheet() -> Rc<StyleSheet> {
    BUTTON_SHEET.with(|s| {
        s.borrow()
            .as_ref()
            .cloned()
            .expect(
                "no Button stylesheet installed; call install_idea_theme(...) before rendering",
            )
    })
}

// =============================================================================
// ButtonSheetBuilder
// =============================================================================

/// Builds a Button [`StyleSheet`] from a list of modifier ZSTs. Starts
/// pre-loaded with the seven built-in tones, four built-in variants,
/// three sizes, and four shapes. Apps can append custom modifiers
/// before calling [`build`](Self::build).
///
/// The resulting sheet has three variant axes:
///
/// - `appearance` — one arm per `(tone, variant)` combination (e.g.
///   `"primary_solid"`, `"hype_outlined"`). The arm's StyleRules come
///   from `variant.render(ctx)` resolved against the tone.
/// - `size` — one arm per ButtonSize. Sets padding + font-size.
/// - `shape` — one arm per Shape. Sets border-radius (all 4 corners).
///
/// Custom modifiers compose with the built-ins automatically: adding
/// `Hype` tone adds 4 new appearance arms (`hype_filled`, `hype_soft`,
/// `hype_outlined`, `hype_ghost`). Adding `Elevated` variant adds 7
/// new ones (`primary_elevated`, `secondary_elevated`, …).
pub struct ButtonSheetBuilder {
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
    sizes: Vec<ButtonSizeRef>,
    shapes: Vec<ShapeRef>,
}

impl ButtonSheetBuilder {
    /// Start with the built-in modifier set (7 tones × 4 variants × 3
    /// sizes × 4 shapes = 336 arms).
    pub fn new() -> Self {
        Self {
            tones: ToneRef::builtins().into_iter().map(|(_, t)| t).collect(),
            variants: VariantRef::builtins().into_iter().map(|(_, v)| v).collect(),
            sizes: ButtonSizeRef::builtins().into_iter().map(|(_, s)| s).collect(),
            shapes: ShapeRef::builtins().into_iter().map(|(_, s)| s).collect(),
        }
    }

    pub fn add_tone(mut self, t: impl Into<ToneRef>) -> Self {
        self.tones.push(t.into());
        self
    }
    pub fn add_variant(mut self, v: impl Into<VariantRef>) -> Self {
        self.variants.push(v.into());
        self
    }
    pub fn add_size(mut self, s: impl Into<ButtonSizeRef>) -> Self {
        self.sizes.push(s.into());
        self
    }
    pub fn add_shape(mut self, s: impl Into<ShapeRef>) -> Self {
        self.shapes.push(s.into());
        self
    }

    /// Construct the stylesheet. The framework pre-generates every
    /// (appearance × size × shape) combination on first apply-style.
    pub fn build(self) -> Rc<StyleSheet> {
        // Base — uniform Button properties + transitions (the visual
        // animation on hover/press/theme-swap).
        let base = StyleSheet::new(|_vs: &VariantSet| StyleRules {
            font_weight: Some(FontWeight::SemiBold),
            letter_spacing: Some(Tokenized::Literal(0.2)),
            text_align: Some(TextAlign::Center),
            background_transition: Some(Transition::new(150, Easing::EaseOut)),
            color_transition: Some(Transition::new(200, Easing::EaseOut)),
            opacity_transition: Some(Transition::new(200, Easing::EaseOut)),
            border_top_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_right_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_bottom_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_left_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            ..Default::default()
        });

        let mut sheet = base;

        // Appearance axis — one arm per (tone, variant) pair. The
        // closure runs ONCE per arm during framework pregen, calling
        // variant.render(ctx) against the active theme to produce the
        // StyleRules for that specific (tone, variant) combo.
        for tone in &self.tones {
            for variant in &self.variants {
                let key = format!("{}_{}", tone.current_key(), variant.current_key());
                let tone_c = tone.clone();
                let variant_c = variant.clone();
                sheet = sheet.variant("appearance", key, move |_vs| {
                    let theme_rc = active_theme();
                    let theme_ref = theme_rc
                        .downcast_ref::<IdeaThemeRef>()
                        .expect("ButtonSheetBuilder closure: install_idea_theme(...) first");
                    let ctx = ResolutionCtx {
                        theme: theme_ref,
                        tone: &*tone_c.0,
                    };
                    variant_c.0.render(&ctx)
                });
            }
        }

        // Size axis — padding + font-size per size.
        for size in &self.sizes {
            let s = size.clone();
            sheet = sheet.variant("size", size.current_key(), move |_vs| {
                let p_v = s.0.padding_vertical();
                let p_h = s.0.padding_horizontal();
                StyleRules {
                    padding_top: Some(p_v.clone()),
                    padding_bottom: Some(p_v),
                    padding_left: Some(p_h.clone()),
                    padding_right: Some(p_h),
                    font_size: Some(s.0.font_size()),
                    ..Default::default()
                }
            });
        }

        // Shape axis — border-radius.
        for shape in &self.shapes {
            let sh = shape.clone();
            sheet = sheet.variant("shape", shape.current_key(), move |_vs| {
                let r = sh.0.border_radius();
                StyleRules {
                    border_top_left_radius: Some(r.clone()),
                    border_top_right_radius: Some(r.clone()),
                    border_bottom_left_radius: Some(r.clone()),
                    border_bottom_right_radius: Some(r),
                    ..Default::default()
                }
            });
        }

        // Defaults so an unset axis applies the most common arm.
        sheet = sheet
            .variant_default("appearance", "primary_filled")
            .variant_default("size", "md")
            .variant_default("shape", "md");

        Rc::new(sheet)
    }
}

impl Default for ButtonSheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Install the default Button stylesheet (built-in modifiers only).
/// Called from `install_idea_theme` so apps that don't need custom
/// modifiers don't have to touch sheet installation.
pub fn install_default_button_sheet() {
    install_button_sheet(ButtonSheetBuilder::new().build());
}

// =============================================================================
// BadgeSheetBuilder / TagSheetBuilder / AlertSheetBuilder
// =============================================================================
//
// These three components share a structure: Tone + Variant only, no
// Size/Shape axes (the component's intrinsic dimensions live in the
// base StyleRules). One internal helper builds the sheet from a
// caller-supplied base.

thread_local! {
    static BADGE_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
    static TAG_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
    static ALERT_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
}

pub fn install_badge_sheet(sheet: Rc<StyleSheet>) {
    BADGE_SHEET.with(|s| *s.borrow_mut() = Some(sheet));
}
pub fn installed_badge_sheet() -> Rc<StyleSheet> {
    BADGE_SHEET.with(|s| {
        s.borrow()
            .as_ref()
            .cloned()
            .expect("no Badge stylesheet installed; call install_idea_theme(...) before rendering")
    })
}

pub fn install_tag_sheet(sheet: Rc<StyleSheet>) {
    TAG_SHEET.with(|s| *s.borrow_mut() = Some(sheet));
}
pub fn installed_tag_sheet() -> Rc<StyleSheet> {
    TAG_SHEET.with(|s| {
        s.borrow()
            .as_ref()
            .cloned()
            .expect("no Tag stylesheet installed; call install_idea_theme(...) before rendering")
    })
}

pub fn install_alert_sheet(sheet: Rc<StyleSheet>) {
    ALERT_SHEET.with(|s| *s.borrow_mut() = Some(sheet));
}
pub fn installed_alert_sheet() -> Rc<StyleSheet> {
    ALERT_SHEET.with(|s| {
        s.borrow()
            .as_ref()
            .cloned()
            .expect("no Alert stylesheet installed; call install_idea_theme(...) before rendering")
    })
}

/// Common builder for "tone × variant only" sheets (Badge, Tag,
/// Alert). The caller passes the base closure (component-specific
/// padding/font/radius). The builder generates `appearance` arms for
/// each `(tone, variant)` pair.
fn build_tone_variant_sheet<B>(
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
    base: B,
) -> Rc<StyleSheet>
where
    B: Fn(&VariantSet) -> StyleRules + 'static,
{
    let mut sheet = StyleSheet::new(base);
    for tone in &tones {
        for variant in &variants {
            let key = format!("{}_{}", tone.current_key(), variant.current_key());
            let tone_c = tone.clone();
            let variant_c = variant.clone();
            sheet = sheet.variant("appearance", key, move |_vs| {
                let theme_rc = active_theme();
                let theme_ref = theme_rc
                    .downcast_ref::<IdeaThemeRef>()
                    .expect("Sheet closure: install_idea_theme(...) first");
                let ctx = ResolutionCtx {
                    theme: theme_ref,
                    tone: &*tone_c.0,
                };
                variant_c.0.render(&ctx)
            });
        }
    }
    sheet = sheet.variant_default("appearance", "neutral_soft");
    Rc::new(sheet)
}

/// Builder for the Badge component's stylesheet.
pub struct BadgeSheetBuilder {
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
}
impl BadgeSheetBuilder {
    pub fn new() -> Self {
        Self {
            tones: ToneRef::builtins().into_iter().map(|(_, t)| t).collect(),
            variants: VariantRef::builtins().into_iter().map(|(_, v)| v).collect(),
        }
    }
    pub fn add_tone(mut self, t: impl Into<ToneRef>) -> Self {
        self.tones.push(t.into());
        self
    }
    pub fn add_variant(mut self, v: impl Into<VariantRef>) -> Self {
        self.variants.push(v.into());
        self
    }
    pub fn build(self) -> Rc<StyleSheet> {
        build_tone_variant_sheet(self.tones, self.variants, |_vs: &VariantSet| StyleRules {
            padding_top: Some(Tokenized::Literal(runtime_core::Length::Px(2.0))),
            padding_bottom: Some(Tokenized::Literal(runtime_core::Length::Px(2.0))),
            padding_left: Some(Tokenized::token(
                "spacing-sm",
                runtime_core::Length::Px(8.0),
            )),
            padding_right: Some(Tokenized::token(
                "spacing-sm",
                runtime_core::Length::Px(8.0),
            )),
            border_top_left_radius: Some(Tokenized::token(
                "radius-pill",
                runtime_core::Length::Px(999.0),
            )),
            border_top_right_radius: Some(Tokenized::token(
                "radius-pill",
                runtime_core::Length::Px(999.0),
            )),
            border_bottom_left_radius: Some(Tokenized::token(
                "radius-pill",
                runtime_core::Length::Px(999.0),
            )),
            border_bottom_right_radius: Some(Tokenized::token(
                "radius-pill",
                runtime_core::Length::Px(999.0),
            )),
            font_size: Some(Tokenized::token(
                "typography-caption-size",
                runtime_core::Length::Px(11.0),
            )),
            font_weight: Some(FontWeight::SemiBold),
            letter_spacing: Some(Tokenized::Literal(0.4)),
            text_transform: Some(runtime_core::TextTransform::Uppercase),
            text_align: Some(TextAlign::Center),
            ..Default::default()
        })
    }
}
impl Default for BadgeSheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn install_default_badge_sheet() {
    install_badge_sheet(BadgeSheetBuilder::new().build());
}

/// Builder for the Tag component's stylesheet. Tag uses the same base
/// shape as Badge — pill, small font, uppercase — with slightly
/// different padding to accommodate the optional close affordance.
pub struct TagSheetBuilder {
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
}
impl TagSheetBuilder {
    pub fn new() -> Self {
        Self {
            tones: ToneRef::builtins().into_iter().map(|(_, t)| t).collect(),
            variants: VariantRef::builtins().into_iter().map(|(_, v)| v).collect(),
        }
    }
    pub fn add_tone(mut self, t: impl Into<ToneRef>) -> Self {
        self.tones.push(t.into());
        self
    }
    pub fn add_variant(mut self, v: impl Into<VariantRef>) -> Self {
        self.variants.push(v.into());
        self
    }
    pub fn build(self) -> Rc<StyleSheet> {
        build_tone_variant_sheet(self.tones, self.variants, |_vs: &VariantSet| StyleRules {
            padding_top: Some(Tokenized::Literal(runtime_core::Length::Px(2.0))),
            padding_bottom: Some(Tokenized::Literal(runtime_core::Length::Px(2.0))),
            padding_left: Some(Tokenized::token(
                "spacing-sm",
                runtime_core::Length::Px(8.0),
            )),
            padding_right: Some(Tokenized::token(
                "spacing-sm",
                runtime_core::Length::Px(8.0),
            )),
            border_top_left_radius: Some(Tokenized::token(
                "radius-pill",
                runtime_core::Length::Px(999.0),
            )),
            border_top_right_radius: Some(Tokenized::token(
                "radius-pill",
                runtime_core::Length::Px(999.0),
            )),
            border_bottom_left_radius: Some(Tokenized::token(
                "radius-pill",
                runtime_core::Length::Px(999.0),
            )),
            border_bottom_right_radius: Some(Tokenized::token(
                "radius-pill",
                runtime_core::Length::Px(999.0),
            )),
            font_size: Some(Tokenized::token(
                "typography-caption-size",
                runtime_core::Length::Px(11.0),
            )),
            font_weight: Some(FontWeight::SemiBold),
            letter_spacing: Some(Tokenized::Literal(0.4)),
            text_align: Some(TextAlign::Center),
            gap: Some(Tokenized::token("spacing-xs", runtime_core::Length::Px(4.0))),
            flex_direction: Some(runtime_core::FlexDirection::Row),
            align_items: Some(runtime_core::AlignItems::Center),
            ..Default::default()
        })
    }
}
impl Default for TagSheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn install_default_tag_sheet() {
    install_tag_sheet(TagSheetBuilder::new().build());
}

/// Builder for the Alert component's stylesheet.
pub struct AlertSheetBuilder {
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
}
impl AlertSheetBuilder {
    pub fn new() -> Self {
        Self {
            tones: ToneRef::builtins().into_iter().map(|(_, t)| t).collect(),
            variants: VariantRef::builtins().into_iter().map(|(_, v)| v).collect(),
        }
    }
    pub fn add_tone(mut self, t: impl Into<ToneRef>) -> Self {
        self.tones.push(t.into());
        self
    }
    pub fn add_variant(mut self, v: impl Into<VariantRef>) -> Self {
        self.variants.push(v.into());
        self
    }
    pub fn build(self) -> Rc<StyleSheet> {
        build_tone_variant_sheet(self.tones, self.variants, |_vs: &VariantSet| StyleRules {
            padding_top: Some(Tokenized::token("spacing-md", runtime_core::Length::Px(12.0))),
            padding_bottom: Some(Tokenized::token("spacing-md", runtime_core::Length::Px(12.0))),
            padding_left: Some(Tokenized::token("spacing-lg", runtime_core::Length::Px(16.0))),
            padding_right: Some(Tokenized::token("spacing-lg", runtime_core::Length::Px(16.0))),
            border_top_left_radius: Some(Tokenized::token(
                "radius-md",
                runtime_core::Length::Px(8.0),
            )),
            border_top_right_radius: Some(Tokenized::token(
                "radius-md",
                runtime_core::Length::Px(8.0),
            )),
            border_bottom_left_radius: Some(Tokenized::token(
                "radius-md",
                runtime_core::Length::Px(8.0),
            )),
            border_bottom_right_radius: Some(Tokenized::token(
                "radius-md",
                runtime_core::Length::Px(8.0),
            )),
            flex_direction: Some(runtime_core::FlexDirection::Row),
            justify_content: Some(runtime_core::JustifyContent::SpaceBetween),
            gap: Some(Tokenized::token("spacing-md", runtime_core::Length::Px(12.0))),
            ..Default::default()
        })
    }
}
impl Default for AlertSheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn install_default_alert_sheet() {
    install_alert_sheet(AlertSheetBuilder::new().build());
}

// =============================================================================
// TypographySheetBuilder
// =============================================================================
//
// Three axes: kind (font characteristics), color (text color), align.
// The kind arm pulls font-size/weight/line-height/letter-spacing from
// the TypographyKind trait. The color axis spans neutral (default,
// muted) and tone-driven values (one arm per tone). The align axis
// maps onto the four TextAlign variants.

thread_local! {
    static TYPOGRAPHY_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
}

pub fn install_typography_sheet(sheet: Rc<StyleSheet>) {
    TYPOGRAPHY_SHEET.with(|s| *s.borrow_mut() = Some(sheet));
}
pub fn installed_typography_sheet() -> Rc<StyleSheet> {
    TYPOGRAPHY_SHEET.with(|s| {
        s.borrow().as_ref().cloned().expect(
            "no Typography stylesheet installed; call install_idea_theme(...) before rendering",
        )
    })
}

/// Builder for the Typography component's stylesheet.
pub struct TypographySheetBuilder {
    kinds: Vec<TypographyKindRef>,
    tones: Vec<ToneRef>,
}
impl TypographySheetBuilder {
    pub fn new() -> Self {
        Self {
            kinds: TypographyKindRef::builtins()
                .into_iter()
                .map(|(_, k)| k)
                .collect(),
            tones: ToneRef::builtins().into_iter().map(|(_, t)| t).collect(),
        }
    }
    pub fn add_kind(mut self, k: impl Into<TypographyKindRef>) -> Self {
        self.kinds.push(k.into());
        self
    }
    pub fn add_tone(mut self, t: impl Into<ToneRef>) -> Self {
        self.tones.push(t.into());
        self
    }
    pub fn build(self) -> Rc<StyleSheet> {
        let mut sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules {
            // Color transitions for theme swap.
            color_transition: Some(Transition::new(250, Easing::EaseInOut)),
            ..Default::default()
        });

        // Kind axis — font characteristics.
        for kind in &self.kinds {
            let k = kind.clone();
            sheet = sheet.variant("kind", kind.current_key(), move |_vs| StyleRules {
                font_size: Some(k.0.font_size()),
                font_weight: Some(k.0.font_weight()),
                line_height: Some(k.0.line_height()),
                letter_spacing: Some(k.0.letter_spacing()),
                ..Default::default()
            });
        }

        // Color axis — neutral defaults + tone-driven.
        sheet = sheet.variant("color", "default", |_vs| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc.downcast_ref::<IdeaThemeRef>().expect("theme");
            StyleRules {
                color: Some(theme_ref.colors().text.clone()),
                ..Default::default()
            }
        });
        sheet = sheet.variant("color", "muted", |_vs| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc.downcast_ref::<IdeaThemeRef>().expect("theme");
            StyleRules {
                color: Some(theme_ref.colors().text_muted.clone()),
                ..Default::default()
            }
        });
        for tone in &self.tones {
            let tone_c = tone.clone();
            sheet = sheet.variant("color", tone.current_key(), move |_vs| {
                let theme_rc = active_theme();
                let theme_ref = theme_rc.downcast_ref::<IdeaThemeRef>().expect("theme");
                StyleRules {
                    color: Some(tone_c.0.stroke_fg(theme_ref)),
                    ..Default::default()
                }
            });
        }

        // Align axis.
        sheet = sheet
            .variant("align", "left", |_vs| StyleRules {
                text_align: Some(TextAlign::Left),
                ..Default::default()
            })
            .variant("align", "center", |_vs| StyleRules {
                text_align: Some(TextAlign::Center),
                ..Default::default()
            })
            .variant("align", "right", |_vs| StyleRules {
                text_align: Some(TextAlign::Right),
                ..Default::default()
            })
            .variant("align", "justify", |_vs| StyleRules {
                text_align: Some(TextAlign::Justify),
                ..Default::default()
            });

        sheet = sheet
            .variant_default("kind", "body")
            .variant_default("color", "default")
            .variant_default("align", "left");

        Rc::new(sheet)
    }
}
impl Default for TypographySheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn install_default_typography_sheet() {
    install_typography_sheet(TypographySheetBuilder::new().build());
}

// =============================================================================
// IconButtonSheetBuilder
// =============================================================================
//
// Tone + Variant (appearance axis) + a closed `size` axis (sm/md/lg)
// that sets the square's width/height/padding/font. The size axis is
// hardcoded (not trait-driven) — square dimensions aren't part of the
// `ButtonSize` slot vocabulary.

thread_local! {
    static ICON_BUTTON_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
}

pub fn install_icon_button_sheet(sheet: Rc<StyleSheet>) {
    ICON_BUTTON_SHEET.with(|s| *s.borrow_mut() = Some(sheet));
}
pub fn installed_icon_button_sheet() -> Rc<StyleSheet> {
    ICON_BUTTON_SHEET.with(|s| {
        s.borrow().as_ref().cloned().expect(
            "no IconButton stylesheet installed; call install_idea_theme(...) before rendering",
        )
    })
}

pub struct IconButtonSheetBuilder {
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
}
impl IconButtonSheetBuilder {
    pub fn new() -> Self {
        Self {
            tones: ToneRef::builtins().into_iter().map(|(_, t)| t).collect(),
            variants: VariantRef::builtins().into_iter().map(|(_, v)| v).collect(),
        }
    }
    pub fn add_tone(mut self, t: impl Into<ToneRef>) -> Self {
        self.tones.push(t.into());
        self
    }
    pub fn add_variant(mut self, v: impl Into<VariantRef>) -> Self {
        self.variants.push(v.into());
        self
    }
    pub fn build(self) -> Rc<StyleSheet> {
        use runtime_core::{AlignItems, JustifyContent, Length};
        let mut sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules {
            font_weight: Some(FontWeight::SemiBold),
            text_align: Some(TextAlign::Center),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            background_transition: Some(Transition::new(150, Easing::EaseOut)),
            color_transition: Some(Transition::new(200, Easing::EaseOut)),
            border_top_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_right_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_bottom_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_left_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            ..Default::default()
        });

        // Appearance arms (tone × variant).
        for tone in &self.tones {
            for variant in &self.variants {
                let key = format!("{}_{}", tone.current_key(), variant.current_key());
                let tone_c = tone.clone();
                let variant_c = variant.clone();
                sheet = sheet.variant("appearance", key, move |_vs| {
                    let theme_rc = active_theme();
                    let theme_ref = theme_rc.downcast_ref::<IdeaThemeRef>().expect("theme");
                    let ctx = ResolutionCtx {
                        theme: theme_ref,
                        tone: &*tone_c.0,
                    };
                    variant_c.0.render(&ctx)
                });
            }
        }

        // Size arms — hardcoded square dimensions (closed enum).
        let pill = |px: f32| Tokenized::token("radius-pill", Length::Px(px));
        sheet = sheet
            .variant("size", "sm", move |_vs| StyleRules {
                padding_top: Some(Tokenized::token("spacing-xs", Length::Px(4.0))),
                padding_bottom: Some(Tokenized::token("spacing-xs", Length::Px(4.0))),
                padding_left: Some(Tokenized::token("spacing-xs", Length::Px(4.0))),
                padding_right: Some(Tokenized::token("spacing-xs", Length::Px(4.0))),
                font_size: Some(Tokenized::token(
                    "typography-body-sm-size",
                    Length::Px(13.0),
                )),
                width: Some(Tokenized::Literal(Length::Px(24.0))),
                height: Some(Tokenized::Literal(Length::Px(24.0))),
                border_top_left_radius: Some(pill(999.0)),
                border_top_right_radius: Some(pill(999.0)),
                border_bottom_left_radius: Some(pill(999.0)),
                border_bottom_right_radius: Some(pill(999.0)),
                ..Default::default()
            })
            .variant("size", "md", move |_vs| StyleRules {
                padding_top: Some(Tokenized::token("spacing-sm", Length::Px(8.0))),
                padding_bottom: Some(Tokenized::token("spacing-sm", Length::Px(8.0))),
                padding_left: Some(Tokenized::token("spacing-sm", Length::Px(8.0))),
                padding_right: Some(Tokenized::token("spacing-sm", Length::Px(8.0))),
                font_size: Some(Tokenized::token("typography-body-size", Length::Px(14.0))),
                width: Some(Tokenized::Literal(Length::Px(32.0))),
                height: Some(Tokenized::Literal(Length::Px(32.0))),
                border_top_left_radius: Some(pill(999.0)),
                border_top_right_radius: Some(pill(999.0)),
                border_bottom_left_radius: Some(pill(999.0)),
                border_bottom_right_radius: Some(pill(999.0)),
                ..Default::default()
            })
            .variant("size", "lg", move |_vs| StyleRules {
                padding_top: Some(Tokenized::token("spacing-md", Length::Px(12.0))),
                padding_bottom: Some(Tokenized::token("spacing-md", Length::Px(12.0))),
                padding_left: Some(Tokenized::token("spacing-md", Length::Px(12.0))),
                padding_right: Some(Tokenized::token("spacing-md", Length::Px(12.0))),
                font_size: Some(Tokenized::token(
                    "typography-body-lg-size",
                    Length::Px(18.0),
                )),
                width: Some(Tokenized::Literal(Length::Px(48.0))),
                height: Some(Tokenized::Literal(Length::Px(48.0))),
                border_top_left_radius: Some(pill(999.0)),
                border_top_right_radius: Some(pill(999.0)),
                border_bottom_left_radius: Some(pill(999.0)),
                border_bottom_right_radius: Some(pill(999.0)),
                ..Default::default()
            });

        sheet = sheet
            .variant_default("appearance", "neutral_filled")
            .variant_default("size", "md");

        Rc::new(sheet)
    }
}
impl Default for IconButtonSheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}

pub fn install_default_icon_button_sheet() {
    install_icon_button_sheet(IconButtonSheetBuilder::new().build());
}

