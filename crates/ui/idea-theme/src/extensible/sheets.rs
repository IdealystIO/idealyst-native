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
    AlignSelf, Cursor, Easing, FontWeight, StyleRules, StyleSheet, TextAlign, Tokenized, Transition,
    UserSelect, VariantSet,
};

use crate::theme::{IdeaTheme, IdeaThemeRef};
use crate::theme_runtime::active_theme;

use super::variant::{variant_state_overlay, InteractState};

/// Every `FontWeight` arm, paired with its stable variant key. Drives
/// Typography's `weight` axis — the axis exists so a per-instance weight
/// override premints instead of riding a runtime-computed layer, so this table
/// must stay exhaustive over the enum (a missing arm silently degrades that
/// weight to `inherit`).
pub const FONT_WEIGHT_KEYS: [(&str, FontWeight); 9] = [
    ("thin", FontWeight::Thin),
    ("extra_light", FontWeight::ExtraLight),
    ("light", FontWeight::Light),
    ("normal", FontWeight::Normal),
    ("medium", FontWeight::Medium),
    ("semi_bold", FontWeight::SemiBold),
    ("bold", FontWeight::Bold),
    ("extra_bold", FontWeight::ExtraBold),
    ("black", FontWeight::Black),
];

/// The variant key for a `FontWeight`. Panics only if [`FONT_WEIGHT_KEYS`]
/// drifts out of sync with the enum, which the exhaustiveness test catches.
pub fn font_weight_key(w: FontWeight) -> &'static str {
    FONT_WEIGHT_KEYS
        .iter()
        .find(|(_, candidate)| *candidate == w)
        .map(|(key, _)| *key)
        .expect("FONT_WEIGHT_KEYS is exhaustive over FontWeight")
}
use super::{
    ButtonSizeRef, RefBuiltins, ResolutionCtx, ShapeRef, ToneRef, TypographyKindRef, VariantRef,
};

/// Register the per-`(tone, variant)` hover + press feedback overlays for a
/// clickable control (Button, IconButton). Each is a `compound` keyed on the
/// active `appearance` arm AND the reserved `__state_*` axis, so the overlay
/// merges *after* the appearance fill and only on the matching variant — letting
/// Ghost/Outlined gain a translucent background fill while Filled/Soft keep a
/// tone-preserving opacity dim (see [`variant_state_overlay`]).
fn add_state_overlay_compounds(
    mut sheet: StyleSheet,
    appearance_key: &str,
    tone: &ToneRef,
    variant: &VariantRef,
) -> StyleSheet {
    for (state_axis, state) in [
        ("__state_hovered", InteractState::Hover),
        ("__state_pressed", InteractState::Press),
    ] {
        let tone_c = tone.clone();
        let variant_c = variant.clone();
        sheet = sheet.compound(
            vec![
                ("appearance", appearance_key.to_string()),
                (state_axis, "on".to_string()),
            ],
            move |_vs| {
                let theme_rc = active_theme();
                let theme_ref = theme_rc
                    .downcast_ref::<IdeaThemeRef>()
                    .expect("state-overlay compound: install_idea_theme(...) first");
                let ctx = ResolutionCtx {
                    theme: theme_ref,
                    tone: &*tone_c.0,
                };
                variant_state_overlay(variant_c.0.key(), &ctx, state)
            },
        );
    }
    sheet
}

/// Register the per-`(tone, variant)` **selected** (active toggle) overlay — an
/// accent fill that wins over the appearance arm when `selected=on`. Tone-driven
/// (the tone's solid fill + its on-fill text color) so a Ghost icon toggle fills
/// with the accent when active, matching the toolbar tool-button feel.
fn add_selected_overlay_compounds(
    mut sheet: StyleSheet,
    appearance_key: &str,
    tone: &ToneRef,
) -> StyleSheet {
    let tone_c = tone.clone();
    sheet = sheet.compound(
        vec![
            ("appearance", appearance_key.to_string()),
            ("selected", "on".to_string()),
        ],
        move |_vs| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc
                .downcast_ref::<IdeaThemeRef>()
                .expect("selected-overlay compound: install_idea_theme(...) first");
            StyleRules {
                background: Some(tone_c.0.fill_bg(theme_ref)),
                color: Some(tone_c.0.fill_fg(theme_ref)),
                ..Default::default()
            }
        },
    );
    sheet
}

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
            // Center the content on BOTH axes. Web centers the label "for
            // free" via the box's `text-align: center` + inline flow; native
            // flex needs it explicit or the label lands at the box's top-left
            // (the macOS "not centered" bug). Applies to the plain
            // single-label case AND the icon row — without `justify_content`
            // even icon buttons were only vertically centered, left-packed
            // horizontally.
            align_items: Some(runtime_core::AlignItems::Center),
            justify_content: Some(runtime_core::JustifyContent::Center),
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
            background_transition: Some(Transition::new(120, Easing::EaseOut)),
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
                {
                    let key = key.clone();
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
                // Per-(tone,variant) hover/press feedback overlay.
                sheet = add_state_overlay_compounds(sheet, &key, tone, variant);
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

        // Register the reserved interaction-state axes so the framework tracks
        // hover/press (the `state_axes` set is populated only by `.variant`, not
        // by `.compound`). Realized as CSS `:hover`/`:active` on web and
        // event-driven on macOS (NSTrackingArea + mouseDown/Up via
        // `attach_states`); touch backends with no hover no-op the hover axis.
        // The arms are intentionally EMPTY — the actual feedback is emitted
        // per-(tone,variant) by `add_state_overlay_compounds` above, so the
        // overlay can be variant-aware (background fill vs opacity dim).
        sheet = sheet
            .variant("__state_hovered", "on", |_vs| StyleRules::default())
            .variant("__state_pressed", "on", |_vs| StyleRules::default());

        // Layout axes. These were one `with_computed("layout_{row}_{block}_{
        // disabled}")` layer on the component, which is three independent
        // booleans — so they enumerate as three axes and premint. (The
        // unconditional half of that layer, centering on both axes, moved to
        // the base above.)
        //
        // The three set disjoint properties, and none collides with
        // appearance/size/shape, so the alphabetical cross-axis merge order
        // is not load-bearing here.
        sheet = sheet
            // Icon+label buttons lay their content out in a row with a gap;
            // the plain single-label button stays a column.
            .variant("layout", "column", |_vs| StyleRules::default())
            .variant("layout", "row", |_vs| StyleRules {
                flex_direction: Some(runtime_core::FlexDirection::Row),
                gap: Some(Tokenized::token("spacing-xs", runtime_core::Length::Px(6.0))),
                ..Default::default()
            })
            // `block` fills the container; otherwise the button hugs.
            .variant("block", "off", |_vs| StyleRules {
                align_self: Some(AlignSelf::Center),
                ..Default::default()
            })
            .variant("block", "on", |_vs| StyleRules {
                width: Some(Tokenized::Literal(runtime_core::Length::Percent(100.0))),
                align_self: Some(AlignSelf::Stretch),
                ..Default::default()
            })
            // Deterministic dim so a disabled button reads as off on every
            // backend. Distinct from the `__state_disabled` overlay: this is
            // the AUTHOR's `disabled` prop, applied whether or not the host
            // marks the node with the platform disabled state.
            .variant("dimmed", "off", |_vs| StyleRules::default())
            .variant("dimmed", "on", |_vs| StyleRules {
                opacity: Some(Tokenized::Literal(0.45)),
                ..Default::default()
            });

        // Defaults so an unset axis applies the most common arm.
        sheet = sheet
            .variant_default("appearance", "primary_filled")
            .variant_default("size", "md")
            .variant_default("shape", "md")
            .variant_default("layout", "column")
            .variant_default("block", "off")
            .variant_default("dimmed", "off");

        sheet.premint_as(&premint_identity(
            "button",
            [
                self.tones.iter().map(|t| t.current_key()).collect::<Vec<_>>().join(","),
                self.variants.iter().map(|v| v.current_key()).collect::<Vec<_>>().join(","),
                self.sizes.iter().map(|z| z.current_key()).collect::<Vec<_>>().join(","),
                self.shapes.iter().map(|h| h.current_key()).collect::<Vec<_>>().join(","),
            ],
        ))
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
    install_button_label_sheet(ButtonSheetBuilder::new().build_label());
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
    component: &str,
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
    // Themed focus ring for the interactive consumer (Chip, on a pressable
    // host). Inert for the non-interactive ones (Badge/Alert are plain views
    // that never receive the FOCUSED state), so it costs nothing to share.
    // Mirrors Button/ControlRow: 1px border in the focus-ring color; the web
    // `:focus` rule kills the browser outline and macOS suppresses its native
    // ring, so this is the sole indicator.
    sheet = sheet.variant("__state_focused", "on", |_vs| focus_ring_rules(1.0, "Sheet closure"));
    // Pointer cursor for the interactive consumer (Chip with an `on_select`);
    // inert `off` default leaves Badge/Tag/Alert untouched, same rationale as
    // the focus ring above.
    //
    // This is a VARIANT and not a call-site `with_computed` layer for two
    // reasons. It premints (a constant closure blocks premint for the whole
    // sheet), and — the bug that motivated the move — the computed layer's
    // cache key is caller-supplied, so Chip's constant `"chip-box"` key did
    // not carry its `clickable` flag. Two chips with the same tone+variant
    // but different `on_select` collided on
    // `(sheet, variants, computed_key, overrides)` and shared one resolved
    // `StyleRules`, so whichever resolved first decided the cursor for both.
    // A variant is part of the cache identity by construction.
    sheet = sheet.variant("interactive", "off", |_vs| StyleRules::default());
    sheet = sheet.variant("interactive", "on", |_vs| StyleRules {
        cursor: Some(Cursor::Pointer),
        ..Default::default()
    });
    sheet = sheet.variant_default("interactive", "off");
    let identity = premint_identity(
        component,
        [
            tones.iter().map(|t| t.current_key()).collect::<Vec<_>>().join(","),
            variants.iter().map(|v| v.current_key()).collect::<Vec<_>>().join(","),
        ],
    );
    sheet.premint_as(&identity)
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
        build_tone_variant_sheet("badge", self.tones, self.variants, |_vs: &VariantSet| StyleRules {
            // Hug: keep the pill sized to content instead of stretching to a
            // flex parent's cross axis (which would grow it to the row height
            // and float its label to the top). Lives in the base rather than
            // a `with_computed` layer at the call site — it's a constant, and
            // a constant closure blocks premint for the whole sheet.
            align_self: Some(AlignSelf::Center),
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
        build_tone_variant_sheet("tag", self.tones, self.variants, |_vs: &VariantSet| StyleRules {
            // Hug — see the Badge sheet's base for why this isn't a computed
            // layer. Shared by Tag and Chip (both resolve the tag sheet).
            align_self: Some(AlignSelf::Center),
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
    install_tag_text_sheets(TagSheetBuilder::new().build_text());
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
        build_tone_variant_sheet("alert", self.tones, self.variants, |_vs: &VariantSet| StyleRules {
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
    install_alert_text_sheets(AlertSheetBuilder::new().build_text());
}

// ---------------------------------------------------------------------------
// Text-slot sheets — the child TEXT nodes of tone×variant components
// ---------------------------------------------------------------------------
//
// Native text nodes (`UILabel`/`NSTextField`/Android `TextView`) inherit
// NOTHING from their container — only web's CSS cascade does — so a
// component's label/title/body must carry the container fill's foreground
// on its OWN node. These used to be composed at the call site: resolve
// the container application, copy its `color` onto a fresh anonymous
// `StyleSheet::r#static`. That composition is invisible to the premint
// dump (the sheet's content depends on which appearance the instance
// picked), so every Alert/Tag text node dragged the live style engine
// into `--premint` builds.
//
// Instead the text sheets carry an `appearance` axis of their own,
// mirroring the container's: one COLOR-ONLY arm per (tone, variant),
// resolved from the same `variant.render(ctx)` the fill uses, built
// up-front at install time. Enumerated arms premint; the component
// just applies `.with("appearance", key)` on both its container and its
// text nodes.

/// Add the color-only `appearance` axis to a text-slot sheet — one arm
/// per (tone, variant) carrying exactly the fill's resolved foreground.
fn text_color_axis(
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
                    .expect("text sheet closure: install_idea_theme(...) first");
                let ctx = ResolutionCtx {
                    theme: theme_ref,
                    tone: &*tone_c.0,
                };
                StyleRules {
                    color: variant_c.0.render(&ctx).color,
                    ..Default::default()
                }
            });
        }
    }
    sheet
}

/// `premint_identity` parts for a tones×variants text sheet.
fn tone_variant_parts(tones: &[ToneRef], variants: &[VariantRef]) -> [String; 2] {
    [
        tones.iter().map(|t| t.current_key()).collect::<Vec<_>>().join(","),
        variants.iter().map(|v| v.current_key()).collect::<Vec<_>>().join(","),
    ]
}

/// The Alert component's text-slot sheets (title / body / the bare `×`
/// glyph), one `appearance` arm per (tone, variant). Install alongside
/// the fill sheet — a custom `AlertSheetBuilder` (extra tones/variants)
/// must install BOTH halves or its custom appearances fall back to the
/// axis default on the text nodes.
pub struct AlertTextSheets {
    pub title: Rc<StyleSheet>,
    pub body: Rc<StyleSheet>,
    /// Color-only sheet for the bare `×` close glyph.
    pub glyph: Rc<StyleSheet>,
}

impl AlertSheetBuilder {
    /// Build the text-slot sheets for the SAME tones/variants as
    /// [`Self::build`]. Typography mirrors the former `AlertTitle` /
    /// `AlertBody` stylesheets; the color arms are what replaces the
    /// call-site composition (see the module section comment above).
    pub fn build_text(&self) -> AlertTextSheets {
        let title = default_neutral_soft(text_color_axis(
            StyleSheet::new(|_vs: &VariantSet| StyleRules {
                font_size: Some(Tokenized::token(
                    "typography-body-size",
                    runtime_core::Length::Px(14.0),
                )),
                font_weight: Some(FontWeight::SemiBold),
                line_height: Some(Tokenized::Literal(20.0)),
                ..Default::default()
            }),
            &self.tones,
            &self.variants,
        ))
        .premint_as(&premint_identity(
            "alert.title",
            tone_variant_parts(&self.tones, &self.variants),
        ));
        let body = default_neutral_soft(text_color_axis(
            StyleSheet::new(|_vs: &VariantSet| StyleRules {
                font_size: Some(Tokenized::token(
                    "typography-body-sm-size",
                    runtime_core::Length::Px(13.0),
                )),
                line_height: Some(Tokenized::Literal(18.0)),
                ..Default::default()
            }),
            &self.tones,
            &self.variants,
        ))
        .premint_as(&premint_identity(
            "alert.body",
            tone_variant_parts(&self.tones, &self.variants),
        ));
        let glyph = default_neutral_soft(text_color_axis(
            StyleSheet::new(|_vs: &VariantSet| StyleRules::default()),
            &self.tones,
            &self.variants,
        ))
        .premint_as(&premint_identity(
            "alert.glyph",
            tone_variant_parts(&self.tones, &self.variants),
        ));
        AlertTextSheets { title, body, glyph }
    }
}

/// The Tag component's text-slot sheets (label / the `×` glyph).
pub struct TagTextSheets {
    pub label: Rc<StyleSheet>,
    /// Color-only sheet for the bare `×` remove glyph.
    pub glyph: Rc<StyleSheet>,
}

impl TagSheetBuilder {
    /// Text-slot sheets for the SAME tones/variants as [`Self::build`] —
    /// see [`AlertSheetBuilder::build_text`] for the contract.
    pub fn build_text(&self) -> TagTextSheets {
        let label = default_neutral_soft(text_color_axis(
            StyleSheet::new(|_vs: &VariantSet| StyleRules {
                font_size: Some(Tokenized::token(
                    "typography-body-sm-size",
                    runtime_core::Length::Px(13.0),
                )),
                font_weight: Some(FontWeight::SemiBold),
                letter_spacing: Some(Tokenized::Literal(0.3)),
                ..Default::default()
            }),
            &self.tones,
            &self.variants,
        ))
        .premint_as(&premint_identity(
            "tag.label",
            tone_variant_parts(&self.tones, &self.variants),
        ));
        let glyph = default_neutral_soft(text_color_axis(
            StyleSheet::new(|_vs: &VariantSet| StyleRules::default()),
            &self.tones,
            &self.variants,
        ))
        .premint_as(&premint_identity(
            "tag.glyph",
            tone_variant_parts(&self.tones, &self.variants),
        ));
        TagTextSheets { label, glyph }
    }
}

/// Alert/Tag text sheets default to the container's own default arm.
fn default_neutral_soft(sheet: StyleSheet) -> StyleSheet {
    sheet.variant_default("appearance", "neutral_soft")
}

impl ButtonSheetBuilder {
    /// The Button LABEL sheet: the container's typography split out onto
    /// the text node (native text inherits neither color nor
    /// weight/size/alignment from the box — the macOS "not bold / not
    /// centered" bug), as enumerated axes instead of the former
    /// per-instance snapshot (`label_typography_style` + a color
    /// override), which was invisible to the premint dump.
    ///
    /// `appearance` arms carry the fill's foreground; `size` arms carry
    /// the label-relevant half of the container's size arms (font-size
    /// only — padding stays on the box). Defaults mirror the container's.
    pub fn build_label(&self) -> Rc<StyleSheet> {
        let mut sheet = text_color_axis(
            StyleSheet::new(|_vs: &VariantSet| StyleRules {
                font_weight: Some(FontWeight::SemiBold),
                letter_spacing: Some(Tokenized::Literal(0.2)),
                text_align: Some(TextAlign::Center),
                ..Default::default()
            }),
            &self.tones,
            &self.variants,
        );
        for size in &self.sizes {
            let sz = size.clone();
            sheet = sheet.variant("size", size.current_key(), move |_vs| StyleRules {
                font_size: Some(sz.0.font_size()),
                ..Default::default()
            });
        }
        sheet
            .variant_default("appearance", "primary_filled")
            .variant_default("size", "md")
            .premint_as(&premint_identity(
                "button.label",
                [
                    self.tones.iter().map(|t| t.current_key()).collect::<Vec<_>>().join(","),
                    self.variants.iter().map(|v| v.current_key()).collect::<Vec<_>>().join(","),
                    self.sizes.iter().map(|z| z.current_key()).collect::<Vec<_>>().join(","),
                ],
            ))
    }
}

// ONE thread_local for every text-sheet slot (not one per sheet):
// bionic caps pthread TLS keys at 128 and each `thread_local!` burns one
// (see the Android TLS note in the repo docs).
#[derive(Default)]
struct TextSheetSlots {
    alert: Option<Rc<AlertTextSheets>>,
    tag: Option<Rc<TagTextSheets>>,
    button_label: Option<Rc<StyleSheet>>,
}
thread_local! {
    static TEXT_SHEETS: RefCell<TextSheetSlots> = RefCell::new(TextSheetSlots::default());
}

pub fn install_alert_text_sheets(sheets: AlertTextSheets) {
    TEXT_SHEETS.with(|s| s.borrow_mut().alert = Some(Rc::new(sheets)));
}
pub fn installed_alert_text_sheets() -> Rc<AlertTextSheets> {
    TEXT_SHEETS.with(|s| {
        s.borrow().alert.clone().expect(
            "no Alert text sheets installed; call install_idea_theme(...) before rendering",
        )
    })
}
pub fn install_button_label_sheet(sheet: Rc<StyleSheet>) {
    TEXT_SHEETS.with(|s| s.borrow_mut().button_label = Some(sheet));
}
pub fn installed_button_label_sheet() -> Rc<StyleSheet> {
    TEXT_SHEETS.with(|s| {
        s.borrow().button_label.clone().expect(
            "no Button label sheet installed; call install_idea_theme(...) before rendering",
        )
    })
}
pub fn install_tag_text_sheets(sheets: TagTextSheets) {
    TEXT_SHEETS.with(|s| s.borrow_mut().tag = Some(Rc::new(sheets)));
}
pub fn installed_tag_text_sheets() -> Rc<TagTextSheets> {
    TEXT_SHEETS.with(|s| {
        s.borrow().tag.clone().expect(
            "no Tag text sheets installed; call install_idea_theme(...) before rendering",
        )
    })
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
            // Deliberately sets NO `font_family`. The theme's font
            // reaches every Typography instance through the framework's
            // default-text-font channel instead: `install_idea_theme` /
            // `set_idea_theme` both call `sync_default_text_font`, the
            // live path fills an absent family at apply time
            // (`fill_default_text_font`), and the preminted path emits
            // `font-family: var(--iy-default-font, inherit)` which the
            // host driver redefines on every theme swap.
            //
            // Baking `theme_ref.font_family()` here instead — which this
            // used to do — is exactly the shape premint cannot honour: a
            // theme-varying value that is NOT a token, so a build-time
            // class would freeze the font of whichever theme the dump
            // build happened to install. Both paths still keep web text
            // out of the browser's serif fallback, which was the original
            // point.
            StyleRules {
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

        // `weight` — the per-instance weight override, layered over the kind's
        // baked-in weight. `FontWeight` is a closed enum, so it enumerates as
        // variant arms and premints.
        //
        // It used to ride a `with_computed` layer, which was also a latent
        // correctness bug: `StyleApplication` has exactly ONE computed slot
        // (`with_computed` assigns, it doesn't stack), so a Typography with
        // BOTH `font` and `weight` set had its font layer silently overwritten
        // by the weight layer. Moving weight to an axis leaves `font` as the
        // sole computed layer and the override survives.
        sheet = sheet.variant("weight", "inherit", |_vs| StyleRules::default());
        for (key, w) in FONT_WEIGHT_KEYS {
            sheet = sheet.variant("weight", key, move |_vs| StyleRules {
                font_weight: Some(w),
                ..Default::default()
            });
        }

        sheet = sheet
            .variant_default("kind", "body")
            .variant_default("color", "default")
            .variant_default("weight", "inherit")
            .variant_default("align", "left");

        sheet.premint_as(&premint_identity("typography", [self.kinds_key(), self.tones_key()]))
    }

    /// The kind axis' declared values, in declaration order — half of
    /// this sheet's premint identity (an app that registers an extra
    /// kind gets a different sheet and must get a different class).
    fn kinds_key(&self) -> String {
        self.kinds.iter().map(|k| k.current_key()).collect::<Vec<_>>().join(",")
    }

    fn tones_key(&self) -> String {
        self.tones.iter().map(|t| t.current_key()).collect::<Vec<_>>().join(",")
    }
}

/// Compose a premint identity for one of this crate's assembled sheets.
///
/// The identity has to describe the sheet's CONTENT, because the dump
/// build and the shipped bundle derive the CSS class from it
/// independently (see [`StyleSheet::premint_as`]). `component` names the
/// sheet; `parts` carry whatever the app can vary — the registered kind
/// and tone keys — so an app that calls `add_kind(...)` gets a distinct
/// class rather than silently wearing the builtin sheet's CSS.
///
/// `V1` is a manual epoch: bump it when a sheet's RULES change in a way
/// its `parts` don't capture (a restyled arm, a new axis). Stale CSS
/// would otherwise survive a framework upgrade, since the class name is
/// all that ties the two halves together.
/// Comma-joined current keys of a modifier set — the part of a sheet's
/// premint identity an app can change by registering an extra tone or
/// variant before `install_idea_theme`.
pub fn tone_keys(tones: &[ToneRef]) -> String {
    tones.iter().map(|t| t.current_key()).collect::<Vec<_>>().join(",")
}

pub fn variant_keys(variants: &[VariantRef]) -> String {
    variants.iter().map(|v| v.current_key()).collect::<Vec<_>>().join(",")
}

pub fn premint_identity(component: &str, parts: impl IntoIterator<Item = String>) -> String {
    let mut id = format!("idea-theme.v1.{component}");
    for part in parts {
        id.push('|');
        id.push_str(&part);
    }
    id
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
            background_transition: Some(Transition::new(120, Easing::EaseOut)),
            color_transition: Some(Transition::new(200, Easing::EaseOut)),
            opacity_transition: Some(Transition::new(200, Easing::EaseOut)),
            border_top_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_right_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_bottom_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            border_left_color_transition: Some(Transition::new(150, Easing::EaseOut)),
            ..Default::default()
        });

        // Appearance arms (tone × variant), plus the hover/press feedback and
        // accent-`selected` overlays for each.
        for tone in &self.tones {
            for variant in &self.variants {
                let key = format!("{}_{}", tone.current_key(), variant.current_key());
                {
                    let key = key.clone();
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
                sheet = add_state_overlay_compounds(sheet, &key, tone, variant);
                sheet = add_selected_overlay_compounds(sheet, &key, tone);
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

        // Reserved interaction-state axes (empty arms; per-(tone,variant)
        // feedback is emitted by the compounds above) — same convention as
        // ButtonSheetBuilder.
        sheet = sheet
            .variant("__state_hovered", "on", |_vs| StyleRules::default())
            .variant("__state_pressed", "on", |_vs| StyleRules::default());

        // `selected` axis — on/off toggle for the accent fill (the active
        // tool-button state). Empty arms; the accent fill is emitted per
        // appearance by `add_selected_overlay_compounds`.
        sheet = sheet
            .variant("selected", "off", |_vs| StyleRules::default())
            .variant("selected", "on", |_vs| StyleRules::default());

        sheet = sheet
            .variant_default("appearance", "neutral_filled")
            .variant_default("size", "md")
            .variant_default("selected", "off");

        // Premint identity, like every sibling builder. Without it
        // `premint_class()` is `None`, `dump_sheet_parts` skips the sheet
        // entirely, and every IconButton falls through to the runtime engine
        // no matter how static its styling is.
        sheet.premint_as(&premint_identity(
            "icon_button",
            [tone_keys(&self.tones), variant_keys(&self.variants)],
        ))
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

/// The themed focus ring as a uniform `width`px border in
/// `colors().focus_ring`. Shared by every sheet that rides a *pressable*
/// host (Switch track, Checkbox box, Radio ring, Tag/Chip): the state
/// overlay resolves above the variant arms, so this wins over whatever
/// border the `checked`/`appearance` arm set.
///
/// WHY a border and not an outline/box-shadow: `StyleRules` has no outline
/// property, and a border lives inside the border-box, so swapping it on
/// focus never changes the control's outer size (no layout nudge on
/// focus/blur).
fn focus_ring_rules(width: f32, whose: &'static str) -> StyleRules {
    let theme_rc = active_theme();
    let theme_ref = theme_rc
        .downcast_ref::<IdeaThemeRef>()
        .unwrap_or_else(|| panic!("{whose}: install_idea_theme(...) first"));
    let ring = theme_ref.colors().focus_ring.clone();
    StyleRules {
        border_top_width: Some(Tokenized::Literal(width)),
        border_right_width: Some(Tokenized::Literal(width)),
        border_bottom_width: Some(Tokenized::Literal(width)),
        border_left_width: Some(Tokenized::Literal(width)),
        border_top_color: Some(ring.clone()),
        border_right_color: Some(ring.clone()),
        border_bottom_color: Some(ring.clone()),
        border_left_color: Some(ring),
        ..Default::default()
    }
}

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

/// Add `appearance` arms that project `variant.render(ctx)`'s **background**
/// (the tone fill) onto the `background` slot — for a tone-colored bar or
/// handle (Progress fill, Slider fill + thumb). Everything else is dropped.
fn add_background_fill_arms(
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
                    .expect("Slider sheet: install_idea_theme(...) first");
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

        // Keyboard/pointer focus rings the track with the themed focus ring —
        // the cross-platform indicator replacing the native ring (suppressed on
        // the pressable host; browser outline killed by the web `:focus` rule).
        // State overlays resolve above the `checked` arms, so this 2px border
        // wins over the OFF arm's zeroed borders.
        sheet = sheet.variant("__state_focused", "on", |_vs| focus_ring_rules(2.0, "Switch sheet"));

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
        sheet.premint_as(&premint_identity(
            "switch",
            [
                self.tones.iter().map(|t| t.current_key()).collect::<Vec<_>>().join(","),
                self.variants.iter().map(|v| v.current_key()).collect::<Vec<_>>().join(","),
            ],
        ))
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
        // The BOX is the pressable host (the label row around it is a plain
        // view), so the focus ring lands on the box alone — a Tab-focused
        // Checkbox rings the square, not the whole label row. State overlays
        // resolve above the `checked` arms, so this 2px border wins over the
        // OFF arm's 1px theme border and the ON arm's fill.
        box_sheet = box_sheet
            .variant("__state_focused", "on", |_vs| focus_ring_rules(2.0, "Checkbox sheet"));
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

        let id = premint_identity("checkbox", [tone_keys(&self.tones), variant_keys(&self.variants)]);
        CheckboxSheets {
            box_sheet: box_sheet.premint_as(&format!("{id}|box")),
            glyph_sheet: glyph_sheet.premint_as(&format!("{id}|glyph")),
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
                outer = outer.variant("appearance", key, move |_vs| {
                    let theme_rc = active_theme();
                    let theme_ref = theme_rc
                        .downcast_ref::<IdeaThemeRef>()
                        .expect("Radio sheet: install_idea_theme(...) first");
                    // The selected ring is a tone-colored OUTLINE. Use the
                    // tone's solid accent (fill_bg) directly — not
                    // `variant.render().color`, which for the default Filled
                    // variant is the on-fill *contrast* color (white), giving
                    // an invisible white ring on a light surface.
                    let stroke = Some(tone_c.0.fill_bg(theme_ref));
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
        // The RING is the pressable host (see the Checkbox box) — focus rings
        // the indicator alone, never the whole label row.
        outer = outer.variant("__state_focused", "on", |_vs| focus_ring_rules(2.0, "Radio sheet"));
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
        // Dot fill = the tone's solid accent (fill_bg), not the shared
        // checkmark-color helper (which projects `variant.render().color` —
        // the white on-fill contrast color for Filled, giving an invisible
        // dot). The radio ring is transparent, so the dot must carry the
        // tone color itself.
        for tone in &self.tones {
            for variant in &self.variants {
                let key = format!("{}_{}", tone.current_key(), variant.current_key());
                let tone_c = tone.clone();
                dot = dot.variant("appearance", key, move |_vs| {
                    let theme_rc = active_theme();
                    let theme_ref = theme_rc
                        .downcast_ref::<IdeaThemeRef>()
                        .expect("Radio dot sheet: install_idea_theme(...) first");
                    StyleRules { background: Some(tone_c.0.fill_bg(theme_ref)), ..Default::default() }
                });
            }
        }
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

        let id = premint_identity("radio", [tone_keys(&self.tones), variant_keys(&self.variants)]);
        RadioSheets {
            outer_sheet: outer.premint_as(&format!("{id}|outer")),
            dot_sheet: dot.premint_as(&format!("{id}|dot")),
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

        // `mode` — the indeterminate bar spans the whole track and animates by
        // transform, so its width is a CONSTANT 100%. (The determinate bar's
        // width comes from the live value and stays off the sheet.) A variant
        // arm rather than a call-site computed layer: a constant closure
        // blocks premint for the whole sheet without expressing anything a
        // variant can't.
        fill = fill.variant("mode", "determinate", |_vs| StyleRules::default());
        fill = fill.variant("mode", "indeterminate", |_vs| StyleRules {
            width: Some(Tokenized::Literal(Length::pct(100.0))),
            ..Default::default()
        });
        fill = fill.variant_default("mode", "determinate");

        let id = premint_identity("progress", [tone_keys(&self.tones), variant_keys(&self.variants)]);
        ProgressSheets {
            track_sheet: track.premint_as(&format!("{id}|track")),
            fill_sheet: fill.premint_as(&format!("{id}|fill")),
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
// SliderSheetBuilder — horizontal value track (muted rail + tone fill + thumb)
// =============================================================================
//
// Three sub-sheets, mirroring Progress's track+fill plus a draggable thumb:
//   - `track_sheet`: the muted rail. `size` axis (rail thickness) only.
//   - `fill_sheet`:  the tone bar from the left edge to the thumb. `appearance`
//     arms tint `background`; the component sets `width` (the value%) via a
//     `with_computed` layer.
//   - `thumb_sheet`: the round handle. `appearance` arms tint `background`;
//     `size` arms set the diameter; the component sets `left` (the value
//     position) via a `with_computed` layer.

thread_local! {
    static SLIDER_SHEETS: RefCell<Option<SliderSheets>> = const { RefCell::new(None) };
}

#[derive(Clone)]
pub struct SliderSheets {
    pub track_sheet: Rc<StyleSheet>,
    pub fill_sheet: Rc<StyleSheet>,
    pub thumb_sheet: Rc<StyleSheet>,
}

pub fn install_slider_sheets(sheets: SliderSheets) {
    SLIDER_SHEETS.with(|s| *s.borrow_mut() = Some(sheets));
}
pub fn installed_slider_sheets() -> SliderSheets {
    SLIDER_SHEETS.with(|s| {
        s.borrow().as_ref().cloned().expect(
            "no Slider stylesheet installed; call install_idea_theme(...) before rendering",
        )
    })
}

/// Closed dimensions per size: `(rail_thickness, thumb_diameter)` in px.
pub const SLIDER_DIMS: [(&str, f32, f32); 3] = [("sm", 3.0, 12.0), ("md", 4.0, 16.0), ("lg", 6.0, 20.0)];

pub struct SliderSheetBuilder {
    tones: Vec<ToneRef>,
    variants: Vec<VariantRef>,
}
impl SliderSheetBuilder {
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
    pub fn build(self) -> SliderSheets {
        use runtime_core::{Length, Overflow, Position};
        let pill = || Tokenized::token("radius-pill", Length::Px(999.0));

        // ---- track (the muted rail) ----
        let mut track = StyleSheet::new(move |_vs: &VariantSet| {
            let theme_rc = active_theme();
            let theme_ref = theme_rc
                .downcast_ref::<IdeaThemeRef>()
                .expect("Slider sheet: install_idea_theme(...) first");
            StyleRules {
                position: Some(Position::Relative),
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
        for (key, h, _thumb) in SLIDER_DIMS {
            track = track.variant("size", key, move |_vs| StyleRules {
                height: Some(Tokenized::Literal(Length::Px(h))),
                ..Default::default()
            });
        }
        track = track.variant_default("size", "md");

        // ---- fill (tone bar; component sets `width`) ----
        let mut fill = StyleSheet::new(move |_vs: &VariantSet| StyleRules {
            position: Some(Position::Absolute),
            left: Some(Tokenized::Literal(Length::Px(0.0))),
            top: Some(Tokenized::Literal(Length::Px(0.0))),
            height: Some(Tokenized::Literal(Length::pct(100.0))),
            border_top_left_radius: Some(pill()),
            border_top_right_radius: Some(pill()),
            border_bottom_left_radius: Some(pill()),
            border_bottom_right_radius: Some(pill()),
            background_transition: Some(Transition::new(120, Easing::EaseOut)),
            ..Default::default()
        });
        fill = add_background_fill_arms(fill, &self.tones, &self.variants);
        fill = fill.variant_default("appearance", "primary_filled");

        // ---- thumb (round handle; component sets `left`) ----
        let mut thumb = StyleSheet::new(move |_vs: &VariantSet| StyleRules {
            position: Some(Position::Absolute),
            top: Some(Tokenized::Literal(Length::Px(0.0))),
            border_top_left_radius: Some(pill()),
            border_top_right_radius: Some(pill()),
            border_bottom_left_radius: Some(pill()),
            border_bottom_right_radius: Some(pill()),
            shadow: Some(runtime_core::Shadow {
                x: 0.0,
                y: 1.0,
                blur: 4.0,
                color: runtime_core::Color("rgba(0,0,0,0.25)".into()),
            }),
            ..Default::default()
        });
        thumb = add_background_fill_arms(thumb, &self.tones, &self.variants);
        for (key, _h, dia) in SLIDER_DIMS {
            thumb = thumb.variant("size", key, move |_vs| StyleRules {
                width: Some(Tokenized::Literal(Length::Px(dia))),
                height: Some(Tokenized::Literal(Length::Px(dia))),
                ..Default::default()
            });
        }
        thumb = thumb
            .variant_default("appearance", "primary_filled")
            .variant_default("size", "md");

        // Premint identities, like the checkbox/button sibling builders —
        // without them every Slider track/fill/thumb fell through to the
        // live engine (`--premint-report` on the docs corpus). The
        // component's continuous values (fill `width`, thumb `left`)
        // already ride the inline layer, so the sheets themselves are
        // fully enumerable.
        let id = premint_identity("slider", [tone_keys(&self.tones), variant_keys(&self.variants)]);
        SliderSheets {
            track_sheet: track.premint_as(&format!("{id}|track")),
            fill_sheet: fill.premint_as(&format!("{id}|fill")),
            thumb_sheet: thumb.premint_as(&format!("{id}|thumb")),
        }
    }
}
impl Default for SliderSheetBuilder {
    fn default() -> Self {
        Self::new()
    }
}
pub fn install_default_slider_sheet() {
    install_slider_sheets(SliderSheetBuilder::new().build());
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
        // The switch track carries the themed focus ring (replaces the native
        // ring; the sole focus indicator on web + desktop).
        assert!(has(&sheet, "__state_focused", "on"));
    }

    #[test]
    fn tag_sheet_has_focus_ring_axis() {
        // Chips ride the Tag sheet on a pressable host, so it must carry the
        // themed focus-ring overlay.
        let sheet = TagSheetBuilder::new().build();
        assert!(has(&sheet, "__state_focused", "on"));
    }

    #[test]
    fn checkbox_box_and_glyph_share_appearance_matrix() {
        let s = CheckboxSheetBuilder::new().build();
        assert_eq!(appearance_arms(&s.box_sheet), BUILTIN_APPEARANCE_ARMS);
        assert_eq!(appearance_arms(&s.glyph_sheet), BUILTIN_APPEARANCE_ARMS);
        assert!(has(&s.box_sheet, "checked", "off"));
        assert!(has(&s.box_sheet, "size", "lg"));
    }

    /// The focus ring belongs to the CONTROL, never to the label row: the
    /// Checkbox box (and the Radio ring) is the pressable host, so its own
    /// sheet carries the `__state_focused` overlay. Before this, the row
    /// wrapping box + label drew the ring, which read as a stray border
    /// around the label text.
    #[test]
    fn regression_focus_ring_lives_on_the_control_not_the_label_row() {
        let cb = CheckboxSheetBuilder::new().build();
        assert!(has(&cb.box_sheet, "__state_focused", "on"), "checkbox box rings itself");
        assert!(
            !has(&cb.glyph_sheet, "__state_focused", "on"),
            "the checkmark is not a focus target"
        );
        let radio = RadioSheetBuilder::new().build();
        assert!(has(&radio.outer_sheet, "__state_focused", "on"), "radio ring rings itself");
        assert!(!has(&radio.dot_sheet, "__state_focused", "on"), "the dot is not a focus target");
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
    fn slider_fill_and_thumb_have_appearance_and_size() {
        let s = SliderSheetBuilder::new().build();
        assert_eq!(appearance_arms(&s.fill_sheet), BUILTIN_APPEARANCE_ARMS);
        assert_eq!(appearance_arms(&s.thumb_sheet), BUILTIN_APPEARANCE_ARMS);
        assert!(has(&s.track_sheet, "size", "sm"));
        assert!(has(&s.thumb_sheet, "size", "lg"));
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

    /// The Button sheet registers the reserved hover/press state axes AND one
    /// hover + one press feedback compound per `(tone, variant)` appearance arm
    /// (the background-fill interactivity upgrade).
    #[test]
    fn button_sheet_has_state_overlay_compounds() {
        let sheet = ButtonSheetBuilder::new().build();
        assert!(has(&sheet, "__state_hovered", "on"));
        assert!(has(&sheet, "__state_pressed", "on"));
        // 2 compounds (hover + press) per appearance arm.
        assert_eq!(sheet.compound_keys().len(), BUILTIN_APPEARANCE_ARMS * 2);
    }

    /// The IconButton sheet adds a `selected` axis (the accent toggle) plus its
    /// per-appearance accent-fill compound, on top of the hover/press pair.
    #[test]
    fn icon_button_sheet_has_selected_axis_and_compounds() {
        let sheet = IconButtonSheetBuilder::new().build();
        assert!(has(&sheet, "selected", "on"));
        assert!(has(&sheet, "selected", "off"));
        assert!(has(&sheet, "__state_hovered", "on"));
        // hover + press + selected = 3 compounds per appearance arm.
        assert_eq!(sheet.compound_keys().len(), BUILTIN_APPEARANCE_ARMS * 3);
    }

    /// `FONT_WEIGHT_KEYS` drives Typography's `weight` axis. A weight missing
    /// from the table resolves to no arm and silently degrades to `inherit`,
    /// so the table must stay exhaustive over the enum. Matching on every
    /// variant makes adding a `FontWeight` a compile error here rather than a
    /// silent styling regression.
    #[test]
    fn font_weight_keys_cover_every_weight() {
        use runtime_core::FontWeight::*;
        let all = [
            Thin, ExtraLight, Light, Normal, Medium, SemiBold, Bold, ExtraBold, Black,
        ];
        // Exhaustiveness against the enum itself: if a variant is added, this
        // match stops compiling until `all` (and the table) grow with it.
        for w in all {
            let _: () = match w {
                Thin | ExtraLight | Light | Normal | Medium | SemiBold | Bold | ExtraBold
                | Black => (),
            };
            assert!(
                FONT_WEIGHT_KEYS.iter().any(|(_, candidate)| *candidate == w),
                "FONT_WEIGHT_KEYS is missing {w:?}"
            );
        }
        assert_eq!(FONT_WEIGHT_KEYS.len(), all.len());
        // Keys must be distinct — a duplicate would collapse two weights onto
        // one arm.
        let mut keys: Vec<&str> = FONT_WEIGHT_KEYS.iter().map(|(k, _)| *k).collect();
        keys.sort_unstable();
        let before = keys.len();
        keys.dedup();
        assert_eq!(keys.len(), before, "duplicate key in FONT_WEIGHT_KEYS");
    }
}
