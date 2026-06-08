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
    Cursor, Easing, FontWeight, StyleRules, StyleSheet, TextAlign, Tokenized, Transition,
    UserSelect, VariantSet,
};

use crate::theme::{IdeaTheme, IdeaThemeRef};
use crate::theme_runtime::active_theme;

use super::{
    ButtonSizeRef, RefBuiltins, ResolutionCtx, ShapeRef, ToneRef, TypographyKindRef, VariantRef,
};

/// Resting → hover → press opacity for interactive controls (Button,
/// IconButton). A subtle uniform dim that reads as interactive feedback
/// without shifting the palette; the controls' `opacity_transition`
/// animates between them. Shared so every clickable dims consistently.
const HOVER_OPACITY: f32 = 0.92;
const PRESSED_OPACITY: f32 = 0.85;

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
            // Interaction affordances every button wants: a pointer cursor on
            // desktop/web, and a label that can't be drag-selected. The
            // framework imposes neither on the bare `pressable` primitive — a
            // component library opts in. Touch backends no-op both.
            cursor: Some(Cursor::Pointer),
            user_select: Some(UserSelect::None),
            // Explicit resting opacity so the hover/press dim has a value to
            // animate back TO. On native the state overlay is applied by
            // re-resolving the style, and a backend leaves opacity untouched
            // when it's unset — so without a base `1.0` the un-hover would
            // never restore full opacity (the dim would stick). Web reverts
            // via the cascade regardless; this keeps the two convergent.
            opacity: Some(Tokenized::Literal(1.0)),
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

        // Interaction-state overlays — a uniform opacity dim on hover and
        // press (animated by the base `opacity_transition`). Registered under
        // the reserved `__state_*` axes the framework recognizes: realized as
        // CSS `:hover`/`:active` on web and event-driven on macOS
        // (NSTrackingArea + mouseDown/Up via `attach_states`); touch backends
        // with no hover no-op the hover arm. Disabled is intentionally NOT a
        // state here — it's a variant-like concern handled by the pressable's
        // disabled mechanism, not a hover-comparable overlay.
        sheet = sheet
            .variant("__state_hovered", "on", |_vs| StyleRules {
                opacity: Some(Tokenized::Literal(HOVER_OPACITY)),
                ..Default::default()
            })
            .variant("__state_pressed", "on", |_vs| StyleRules {
                opacity: Some(Tokenized::Literal(PRESSED_OPACITY)),
                ..Default::default()
            });

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
        let mut sheet = StyleSheet::new(|_vs: &VariantSet| {
            // The theme's default font family lands on the base so every
            // Typography instance inherits it. Reads `active_theme()` so
            // a theme swap (which wipes the resolution cache) re-runs
            // this and re-applies the new font. Critically, this keeps
            // web text out of the browser's serif fallback — native
            // backends already default to a system sans.
            let theme_rc = active_theme();
            let theme_ref = theme_rc
                .downcast_ref::<IdeaThemeRef>()
                .expect("Typography sheet: install_idea_theme(...) first");
            StyleRules {
                font_family: Some(theme_ref.font_family()),
                // Color transitions for theme swap.
                color_transition: Some(Transition::new(250, Easing::EaseInOut)),
                ..Default::default()
            }
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
            // Pointer cursor + non-selectable glyph + resting opacity for the
            // hover/press dim to animate back to — see ButtonSheetBuilder.
            cursor: Some(Cursor::Pointer),
            user_select: Some(UserSelect::None),
            opacity: Some(Tokenized::Literal(1.0)),
            background_transition: Some(Transition::new(150, Easing::EaseOut)),
            color_transition: Some(Transition::new(200, Easing::EaseOut)),
            opacity_transition: Some(Transition::new(200, Easing::EaseOut)),
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

        // Hover / press dim — same convention as ButtonSheetBuilder.
        sheet = sheet
            .variant("__state_hovered", "on", |_vs| StyleRules {
                opacity: Some(Tokenized::Literal(HOVER_OPACITY)),
                ..Default::default()
            })
            .variant("__state_pressed", "on", |_vs| StyleRules {
                opacity: Some(Tokenized::Literal(PRESSED_OPACITY)),
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

// =============================================================================
// Shared arm helpers for the selection-control family (Switch, Checkbox, Radio)
// =============================================================================
//
// These three components share one structural idea: an `appearance`
// axis with one arm per `(tone, variant)` pair whose StyleRules come
// from `variant.render(ctx)` — exactly like Button/Badge — plus a
// `checked` axis (on/off) that overrides the appearance when the
// control is unselected. Because the framework merges variant axes in
// alphabetical name order (`appearance` < `checked` < `size`), the
// `checked=off` arm reliably wins over the appearance fill, and the
// `size` arm (dimensions only) wins over both.

/// The neutral "unselected" look, shared by Checkbox box + Radio
/// outer ring: transparent surface, a 1px theme border on every side,
/// and muted foreground. Overrides whatever the `appearance` arm set.
fn unchecked_surface_rules() -> StyleRules {
    let theme_rc = active_theme();
    let theme_ref = theme_rc
        .downcast_ref::<IdeaThemeRef>()
        .expect("selection-control sheet: install_idea_theme(...) first");
    let border = theme_ref.colors().border.clone();
    StyleRules {
        background: Some(Tokenized::Literal(runtime_core::Color("transparent".into()))),
        color: Some(theme_ref.colors().text_muted.clone()),
        border_top_width: Some(Tokenized::Literal(1.0)),
        border_right_width: Some(Tokenized::Literal(1.0)),
        border_bottom_width: Some(Tokenized::Literal(1.0)),
        border_left_width: Some(Tokenized::Literal(1.0)),
        border_top_color: Some(border.clone()),
        border_right_color: Some(border.clone()),
        border_bottom_color: Some(border.clone()),
        border_left_color: Some(border),
        ..Default::default()
    }
}

/// Add `appearance` arms (one per `(tone, variant)`) whose rules come
/// straight from `variant.render(ctx)` — the same selected/filled look
/// Button and Badge use. Custom tones/variants flow through unchanged.
fn add_appearance_arms(
    mut sheet: StyleSheet,
    tones: &[ToneRef],
    variants: &[VariantRef],
) -> StyleSheet {
    for tone in tones {
        for variant in variants {
            let key = format!("{}_{}", tone.current_key(), variant.current_key());
            let tone_c = tone.clone();
            let variant_c = variant.clone();
            sheet = sheet.variant("appearance", key, move |_vs| {
                let theme_rc = active_theme();
                let theme_ref = theme_rc
                    .downcast_ref::<IdeaThemeRef>()
                    .expect("selection-control sheet: install_idea_theme(...) first");
                let ctx = ResolutionCtx {
                    theme: theme_ref,
                    tone: &*tone_c.0,
                };
                variant_c.0.render(&ctx)
            });
        }
    }
    sheet
}

/// Add `appearance` arms that project `variant.render(ctx)`'s
/// foreground color onto a single target — `color` (for a checkmark
/// glyph) when `as_background` is false, or `background` (for a radio
/// dot / a switch's "on" fill marker) when true. Everything else is
/// dropped, so the arm only tints the indicator.
fn add_indicator_color_arms(
    mut sheet: StyleSheet,
    tones: &[ToneRef],
    variants: &[VariantRef],
    as_background: bool,
) -> StyleSheet {
    for tone in tones {
        for variant in variants {
            let key = format!("{}_{}", tone.current_key(), variant.current_key());
            let tone_c = tone.clone();
            let variant_c = variant.clone();
            sheet = sheet.variant("appearance", key, move |_vs| {
                let theme_rc = active_theme();
                let theme_ref = theme_rc
                    .downcast_ref::<IdeaThemeRef>()
                    .expect("selection-control sheet: install_idea_theme(...) first");
                let ctx = ResolutionCtx {
                    theme: theme_ref,
                    tone: &*tone_c.0,
                };
                let fg = variant_c.0.render(&ctx).color;
                if as_background {
                    StyleRules { background: fg, ..Default::default() }
                } else {
                    StyleRules { color: fg, ..Default::default() }
                }
            });
        }
    }
    sheet
}

// =============================================================================
// SwitchSheetBuilder — styled slide-toggle track
// =============================================================================
//
// A Switch is a pill track with a circular thumb that slides between
// the off (left) and on (right) edges. The track's "on" fill is the
// tone/variant render; the "off" fill is a muted theme track. The
// thumb itself carries no tone — it's a white puck styled by an
// idea-ui-local stylesheet — and its horizontal position is animated
// by the component via `AnimProp::TranslateX`.

thread_local! {
    static SWITCH_SHEET: RefCell<Option<Rc<StyleSheet>>> = const { RefCell::new(None) };
}

pub fn install_switch_sheet(sheet: Rc<StyleSheet>) {
    SWITCH_SHEET.with(|s| *s.borrow_mut() = Some(sheet));
}
pub fn installed_switch_sheet() -> Rc<StyleSheet> {
    SWITCH_SHEET.with(|s| {
        s.borrow().as_ref().cloned().expect(
            "no Switch stylesheet installed; call install_idea_theme(...) before rendering",
        )
    })
}

/// Closed track dimensions per size: `(width, height)` in px.
pub const SWITCH_TRACK_DIMS: [(&str, f32, f32); 3] =
    [("sm", 30.0, 18.0), ("md", 38.0, 22.0), ("lg", 48.0, 28.0)];

pub struct SwitchSheetBuilder {
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
}
impl SwitchSheetBuilder {
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
        use runtime_core::{AlignItems, FlexDirection, Length};
        let pill = || Tokenized::token("radius-pill", Length::Px(999.0));
        let mut sheet = StyleSheet::new(move |_vs: &VariantSet| StyleRules {
            flex_direction: Some(FlexDirection::Row),
            align_items: Some(AlignItems::Center),
            border_top_left_radius: Some(pill()),
            border_top_right_radius: Some(pill()),
            border_bottom_left_radius: Some(pill()),
            border_bottom_right_radius: Some(pill()),
            padding_top: Some(Tokenized::Literal(Length::Px(2.0))),
            padding_bottom: Some(Tokenized::Literal(Length::Px(2.0))),
            padding_left: Some(Tokenized::Literal(Length::Px(2.0))),
            padding_right: Some(Tokenized::Literal(Length::Px(2.0))),
            background_transition: Some(Transition::new(180, Easing::EaseOut)),
            ..Default::default()
        });

        // ON look — tone/variant fill.
        sheet = add_appearance_arms(sheet, &self.tones, &self.variants);

        // OFF look — muted track, no border.
        sheet = sheet.variant("checked", "off", |_vs| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc
                .downcast_ref::<IdeaThemeRef>()
                .expect("Switch sheet: install_idea_theme(...) first");
            StyleRules {
                background: Some(theme_ref.colors().border.clone()),
                border_top_width: Some(Tokenized::Literal(0.0)),
                border_right_width: Some(Tokenized::Literal(0.0)),
                border_bottom_width: Some(Tokenized::Literal(0.0)),
                border_left_width: Some(Tokenized::Literal(0.0)),
                ..Default::default()
            }
        });
        sheet = sheet.variant("checked", "on", |_vs| StyleRules::default());

        // Size — track width/height.
        for (key, w, h) in SWITCH_TRACK_DIMS {
            sheet = sheet.variant("size", key, move |_vs| StyleRules {
                width: Some(Tokenized::Literal(Length::Px(w))),
                height: Some(Tokenized::Literal(Length::Px(h))),
                ..Default::default()
            });
        }

        sheet = sheet
            .variant_default("appearance", "primary_filled")
            .variant_default("checked", "off")
            .variant_default("size", "md");
        Rc::new(sheet)
    }
}
impl Default for SwitchSheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}
pub fn install_default_switch_sheet() {
    install_switch_sheet(SwitchSheetBuilder::new().build());
}

// =============================================================================
// CheckboxSheetBuilder — box + checkmark glyph
// =============================================================================
//
// Two sub-sheets, bundled into one install/installed pair:
//   - `box_sheet`: the square. `appearance` (tone×variant, the checked
//     fill) + `checked` (off override) + `size` (dimensions).
//   - `glyph_sheet`: the checkmark text. `appearance` arms tint only
//     the glyph's `color` to the variant foreground; rendered only
//     while checked, so it never needs an off arm.

thread_local! {
    static CHECKBOX_SHEETS: RefCell<Option<CheckboxSheets>> = const { RefCell::new(None) };
}

/// The pair of sheets a Checkbox renders with.
#[derive(Clone)]
pub struct CheckboxSheets {
    pub box_sheet: Rc<StyleSheet>,
    pub glyph_sheet: Rc<StyleSheet>,
}

pub fn install_checkbox_sheets(sheets: CheckboxSheets) {
    CHECKBOX_SHEETS.with(|s| *s.borrow_mut() = Some(sheets));
}
pub fn installed_checkbox_sheets() -> CheckboxSheets {
    CHECKBOX_SHEETS.with(|s| {
        s.borrow().as_ref().cloned().expect(
            "no Checkbox stylesheet installed; call install_idea_theme(...) before rendering",
        )
    })
}

/// Closed box dimensions per size: `(box_px, glyph_font_px)`.
pub const CHECKBOX_DIMS: [(&str, f32, f32); 3] =
    [("sm", 16.0, 11.0), ("md", 20.0, 14.0), ("lg", 24.0, 17.0)];

pub struct CheckboxSheetBuilder {
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
}
impl CheckboxSheetBuilder {
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
    pub fn build(self) -> CheckboxSheets {
        use runtime_core::{AlignItems, JustifyContent, Length};
        let radius = || Tokenized::token("radius-sm", Length::Px(4.0));

        // ---- box ----
        let mut box_sheet = StyleSheet::new(move |_vs: &VariantSet| StyleRules {
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            border_top_left_radius: Some(radius()),
            border_top_right_radius: Some(radius()),
            border_bottom_left_radius: Some(radius()),
            border_bottom_right_radius: Some(radius()),
            background_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_top_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_right_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_bottom_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_left_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            ..Default::default()
        });
        box_sheet = add_appearance_arms(box_sheet, &self.tones, &self.variants);
        box_sheet = box_sheet
            .variant("checked", "off", |_vs| unchecked_surface_rules())
            .variant("checked", "on", |_vs| StyleRules::default());
        for (key, dim, _glyph) in CHECKBOX_DIMS {
            box_sheet = box_sheet.variant("size", key, move |_vs| StyleRules {
                width: Some(Tokenized::Literal(Length::Px(dim))),
                height: Some(Tokenized::Literal(Length::Px(dim))),
                ..Default::default()
            });
        }
        box_sheet = box_sheet
            .variant_default("appearance", "primary_filled")
            .variant_default("checked", "off")
            .variant_default("size", "md");

        // ---- glyph (checkmark) ----
        let mut glyph_sheet = StyleSheet::new(|_vs: &VariantSet| StyleRules {
            font_weight: Some(FontWeight::Bold),
            text_align: Some(TextAlign::Center),
            ..Default::default()
        });
        glyph_sheet = add_indicator_color_arms(glyph_sheet, &self.tones, &self.variants, false);
        for (key, _dim, glyph) in CHECKBOX_DIMS {
            glyph_sheet = glyph_sheet.variant("size", key, move |_vs| StyleRules {
                font_size: Some(Tokenized::Literal(Length::Px(glyph))),
                line_height: Some(Tokenized::Literal(glyph)),
                ..Default::default()
            });
        }
        glyph_sheet = glyph_sheet
            .variant_default("appearance", "primary_filled")
            .variant_default("size", "md");

        CheckboxSheets {
            box_sheet: Rc::new(box_sheet),
            glyph_sheet: Rc::new(glyph_sheet),
        }
    }
}
impl Default for CheckboxSheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}
pub fn install_default_checkbox_sheet() {
    install_checkbox_sheets(CheckboxSheetBuilder::new().build());
}

// =============================================================================
// RadioSheetBuilder — outer ring + inner dot
// =============================================================================
//
// Mirror of Checkbox, but circular and the selected indicator is a
// filled dot (a child view) rather than a glyph. `outer_sheet` is the
// ring (`appearance`/`checked`/`size`); `dot_sheet` tints the inner
// view's `background` to the variant foreground (rendered only while
// selected).

thread_local! {
    static RADIO_SHEETS: RefCell<Option<RadioSheets>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub struct RadioSheets {
    pub outer_sheet: Rc<StyleSheet>,
    pub dot_sheet: Rc<StyleSheet>,
}

pub fn install_radio_sheets(sheets: RadioSheets) {
    RADIO_SHEETS.with(|s| *s.borrow_mut() = Some(sheets));
}
pub fn installed_radio_sheets() -> RadioSheets {
    RADIO_SHEETS.with(|s| {
        s.borrow().as_ref().cloned().expect(
            "no Radio stylesheet installed; call install_idea_theme(...) before rendering",
        )
    })
}

/// Closed dimensions per size: `(outer_px, dot_px)`.
pub const RADIO_DIMS: [(&str, f32, f32); 3] =
    [("sm", 16.0, 8.0), ("md", 20.0, 10.0), ("lg", 24.0, 12.0)];

pub struct RadioSheetBuilder {
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
}
impl RadioSheetBuilder {
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
    pub fn build(self) -> RadioSheets {
        use runtime_core::{AlignItems, JustifyContent, Length};
        let pill = || Tokenized::token("radius-pill", Length::Px(999.0));

        // ---- outer ring ----
        let mut outer = StyleSheet::new(move |_vs: &VariantSet| StyleRules {
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            border_top_left_radius: Some(pill()),
            border_top_right_radius: Some(pill()),
            border_bottom_left_radius: Some(pill()),
            border_bottom_right_radius: Some(pill()),
            background_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_top_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_right_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_bottom_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_left_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            ..Default::default()
        });
        // Radio's selected ring reads best as an outline, not a solid
        // fill — keep the ring transparent with a tone-colored border
        // even when selected, and let the dot carry the fill.
        for tone in &self.tones {
            for variant in &self.variants {
                let key = format!("{}_{}", tone.current_key(), variant.current_key());
                let tone_c = tone.clone();
                let variant_c = variant.clone();
                outer = outer.variant("appearance", key, move |_vs| {
                    let theme_rc = active_theme();
                    let theme_ref = theme_rc
                        .downcast_ref::<IdeaThemeRef>()
                        .expect("Radio sheet: install_idea_theme(...) first");
                    let ctx = ResolutionCtx {
                        theme: theme_ref,
                        tone: &*tone_c.0,
                    };
                    let stroke = variant_c.0.render(&ctx).color;
                    StyleRules {
                        background: Some(Tokenized::Literal(runtime_core::Color(
                            "transparent".into(),
                        ))),
                        border_top_width: Some(Tokenized::Literal(1.0)),
                        border_right_width: Some(Tokenized::Literal(1.0)),
                        border_bottom_width: Some(Tokenized::Literal(1.0)),
                        border_left_width: Some(Tokenized::Literal(1.0)),
                        border_top_color: stroke.clone(),
                        border_right_color: stroke.clone(),
                        border_bottom_color: stroke.clone(),
                        border_left_color: stroke,
                        ..Default::default()
                    }
                });
            }
        }
        outer = outer
            .variant("checked", "off", |_vs| unchecked_surface_rules())
            .variant("checked", "on", |_vs| StyleRules::default());
        for (key, dim, _dot) in RADIO_DIMS {
            outer = outer.variant("size", key, move |_vs| StyleRules {
                width: Some(Tokenized::Literal(Length::Px(dim))),
                height: Some(Tokenized::Literal(Length::Px(dim))),
                ..Default::default()
            });
        }
        outer = outer
            .variant_default("appearance", "primary_filled")
            .variant_default("checked", "off")
            .variant_default("size", "md");

        // ---- inner dot ----
        let mut dot = StyleSheet::new(move |_vs: &VariantSet| StyleRules {
            border_top_left_radius: Some(pill()),
            border_top_right_radius: Some(pill()),
            border_bottom_left_radius: Some(pill()),
            border_bottom_right_radius: Some(pill()),
            background_transition: Some(Transition::new(150, Easing::EaseOut)),
            ..Default::default()
        });
        dot = add_indicator_color_arms(dot, &self.tones, &self.variants, true);
        for (key, _dim, dot_px) in RADIO_DIMS {
            dot = dot.variant("size", key, move |_vs| StyleRules {
                width: Some(Tokenized::Literal(Length::Px(dot_px))),
                height: Some(Tokenized::Literal(Length::Px(dot_px))),
                ..Default::default()
            });
        }
        dot = dot
            .variant_default("appearance", "primary_filled")
            .variant_default("size", "md");

        RadioSheets {
            outer_sheet: Rc::new(outer),
            dot_sheet: Rc::new(dot),
        }
    }
}
impl Default for RadioSheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}
pub fn install_default_radio_sheet() {
    install_radio_sheets(RadioSheetBuilder::new().build());
}

// =============================================================================
// ProgressSheetBuilder — linear bar (muted track + tone fill)
// =============================================================================
//
// Two sub-sheets:
//   - `track_sheet`: the muted rail. `size` axis (bar thickness) only.
//   - `fill_sheet`: the tone bar. `appearance` arms tint `background`
//     to the variant fill; the component sets `width` (the value%)
//     via a `with_computed` layer.

thread_local! {
    static PROGRESS_SHEETS: RefCell<Option<ProgressSheets>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub struct ProgressSheets {
    pub track_sheet: Rc<StyleSheet>,
    pub fill_sheet: Rc<StyleSheet>,
}

pub fn install_progress_sheets(sheets: ProgressSheets) {
    PROGRESS_SHEETS.with(|s| *s.borrow_mut() = Some(sheets));
}
pub fn installed_progress_sheets() -> ProgressSheets {
    PROGRESS_SHEETS.with(|s| {
        s.borrow().as_ref().cloned().expect(
            "no Progress stylesheet installed; call install_idea_theme(...) before rendering",
        )
    })
}

/// Bar thickness (px) per size.
pub const PROGRESS_DIMS: [(&str, f32); 3] = [("sm", 4.0), ("md", 8.0), ("lg", 12.0)];

pub struct ProgressSheetBuilder {
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
}
impl ProgressSheetBuilder {
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
    pub fn build(self) -> ProgressSheets {
        use runtime_core::{Length, Overflow};
        let pill = || Tokenized::token("radius-pill", Length::Px(999.0));

        // ---- track ----
        let mut track = StyleSheet::new(move |_vs: &VariantSet| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc
                .downcast_ref::<IdeaThemeRef>()
                .expect("Progress sheet: install_idea_theme(...) first");
            StyleRules {
                background: Some(theme_ref.colors().border.clone()),
                width: Some(Tokenized::Literal(Length::pct(100.0))),
                overflow: Some(Overflow::Hidden),
                border_top_left_radius: Some(pill()),
                border_top_right_radius: Some(pill()),
                border_bottom_left_radius: Some(pill()),
                border_bottom_right_radius: Some(pill()),
                ..Default::default()
            }
        });
        for (key, h) in PROGRESS_DIMS {
            track = track.variant("size", key, move |_vs| StyleRules {
                height: Some(Tokenized::Literal(Length::Px(h))),
                ..Default::default()
            });
        }
        track = track.variant_default("size", "md");

        // ---- fill ----
        let mut fill = StyleSheet::new(move |_vs: &VariantSet| StyleRules {
            height: Some(Tokenized::Literal(Length::pct(100.0))),
            border_top_left_radius: Some(pill()),
            border_top_right_radius: Some(pill()),
            border_bottom_left_radius: Some(pill()),
            border_bottom_right_radius: Some(pill()),
            background_transition: Some(Transition::new(200, Easing::EaseOut)),
            opacity_transition: Some(Transition::new(200, Easing::EaseOut)),
            ..Default::default()
        });
        for tone in &self.tones {
            for variant in &self.variants {
                let key = format!("{}_{}", tone.current_key(), variant.current_key());
                let tone_c = tone.clone();
                let variant_c = variant.clone();
                fill = fill.variant("appearance", key, move |_vs| {
                    let theme_rc = active_theme();
                    let theme_ref = theme_rc
                        .downcast_ref::<IdeaThemeRef>()
                        .expect("Progress sheet: install_idea_theme(...) first");
                    let ctx = ResolutionCtx {
                        theme: theme_ref,
                        tone: &*tone_c.0,
                    };
                    StyleRules {
                        background: variant_c.0.render(&ctx).background,
                        ..Default::default()
                    }
                });
            }
        }
        fill = fill.variant_default("appearance", "primary_filled");

        ProgressSheets {
            track_sheet: Rc::new(track),
            fill_sheet: Rc::new(fill),
        }
    }
}
impl Default for ProgressSheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}
pub fn install_default_progress_sheet() {
    install_progress_sheets(ProgressSheetBuilder::new().build());
}


// =============================================================================
// Tests — selection-control + progress sheet builders
// =============================================================================

#[cfg(test)]
mod selection_sheet_tests {
    use super::*;

    /// Count the `appearance` arms a built sheet declares — one per
    /// `(tone, variant)` pair. The closures aren't run (no theme
    /// needed), so this just verifies the Cartesian product wiring.
    fn appearance_arms(sheet: &StyleSheet) -> usize {
        sheet
            .variant_keys()
            .iter()
            .filter(|(axis, _)| axis == "appearance")
            .count()
    }

    fn has(sheet: &StyleSheet, axis: &str, value: &str) -> bool {
        sheet
            .variant_keys()
            .iter()
            .any(|(a, v)| a == axis && v == value)
    }

    const BUILTIN_APPEARANCE_ARMS: usize = 7 * 4; // 7 tones × 4 variants

    #[test]
    fn switch_sheet_has_builtin_arms_and_axes() {
        let sheet = SwitchSheetBuilder::new().build();
        assert_eq!(appearance_arms(&sheet), BUILTIN_APPEARANCE_ARMS);
        assert!(has(&sheet, "appearance", "primary_filled"));
        assert!(has(&sheet, "checked", "off"));
        assert!(has(&sheet, "checked", "on"));
        assert!(has(&sheet, "size", "md"));
    }

    #[test]
    fn checkbox_box_and_glyph_share_appearance_matrix() {
        let s = CheckboxSheetBuilder::new().build();
        assert_eq!(appearance_arms(&s.box_sheet), BUILTIN_APPEARANCE_ARMS);
        assert_eq!(appearance_arms(&s.glyph_sheet), BUILTIN_APPEARANCE_ARMS);
        assert!(has(&s.box_sheet, "checked", "off"));
        assert!(has(&s.box_sheet, "size", "lg"));
    }

    #[test]
    fn radio_outer_and_dot_share_appearance_matrix() {
        let s = RadioSheetBuilder::new().build();
        assert_eq!(appearance_arms(&s.outer_sheet), BUILTIN_APPEARANCE_ARMS);
        assert_eq!(appearance_arms(&s.dot_sheet), BUILTIN_APPEARANCE_ARMS);
        assert!(has(&s.outer_sheet, "checked", "off"));
    }

    #[test]
    fn progress_fill_has_appearance_track_has_size() {
        let s = ProgressSheetBuilder::new().build();
        assert_eq!(appearance_arms(&s.fill_sheet), BUILTIN_APPEARANCE_ARMS);
        assert!(has(&s.track_sheet, "size", "sm"));
        assert!(has(&s.track_sheet, "size", "lg"));
    }

    #[test]
    fn add_tone_extends_the_appearance_matrix_by_one_variant_row() {
        // A custom tone adds one arm per variant (×4) on top of builtins.
        let base = SwitchSheetBuilder::new().build();
        let extended = SwitchSheetBuilder::new()
            .add_tone(crate::extensible::tone::Primary) // stand-in custom tone
            .build();
        // Primary already exists, so a duplicate key dedupes — the count
        // is unchanged. This guards against the builder silently dropping
        // the builtin set when a tone is appended.
        assert_eq!(appearance_arms(&base), appearance_arms(&extended));
    }
}
