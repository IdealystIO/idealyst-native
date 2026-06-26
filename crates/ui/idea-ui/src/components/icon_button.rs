//! `IconButton` — square clickable for a glyph, built on the
//! extensible Tone + Variant trait surface.
//!
//! ```ignore
//! use idea_ui::extensible::icon_button::{icon_button, IconButtonProps, IconButtonSize};
//! use idea_theme::extensible::{tone, variant};
//!
//! ui! {
//!     IconButton(
//!         glyph = "×",
//!         on_click = on_dismiss,
//!         tone = tone::Neutral,
//!         variant = variant::Ghost,
//!         size = IconButtonSize::Md,
//!     )
//! }
//! ```
//!
//! Tone + Variant are extensible (trait objects); `size` stays a
//! closed enum because it controls the square's width/height — a
//! continuous extension would require additional theme tokens that
//! aren't part of the `ButtonSize` slot vocabulary.

use std::rc::Rc;

use runtime_core::{
    component, icon, text, AlignSelf, Color, Element, IconData, IdealystSchema, IntoElement, Length,
    Reactive, StyleApplication, StyleRules, StyleSheet, Tokenized, VariantEnum,
};

use idea_theme::extensible::{installed_icon_button_sheet, tone, variant, ToneRef, VariantRef};

pub use crate::stylesheets::IconButtonSize;

/// Glyph-area icon size per IconButtonSize step. The square is
/// 24/32/48 px (Sm/Md/Lg) with padding, so the icon fills the content
/// box without crowding the padding.
fn icon_px_for(size: IconButtonSize) -> f32 {
    match size.as_variant_str() {
        "sm" => 16.0,
        "lg" => 24.0,
        // "md" and any future fallback.
        _ => 18.0,
    }
}

thread_local! {
    static ICON_BUTTON_ICON_SHEETS: std::cell::RefCell<
        std::collections::HashMap<u32, Rc<StyleSheet>>,
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// Cached static sheet pinning the icon to a `px × px` square. Icons
/// have no intrinsic content size, so without an explicit width/height
/// they collapse to 0×0 inside the centered square.
fn icon_button_icon_sheet(px: f32) -> Rc<StyleSheet> {
    let key = (px * 100.0).round() as u32;
    ICON_BUTTON_ICON_SHEETS.with(|m| {
        if let Some(s) = m.borrow().get(&key) {
            return s.clone();
        }
        let sheet = Rc::new(StyleSheet::r#static(StyleRules {
            width: Some(Tokenized::Literal(Length::Px(px))),
            height: Some(Tokenized::Literal(Length::Px(px))),
            flex_shrink: Some(Tokenized::Literal(0.0)),
            ..Default::default()
        }));
        m.borrow_mut().insert(key, sheet.clone());
        sheet
    })
}

// Reactive-by-default: `#[props]` wraps each data field `T` → `Reactive<T>`;
// `on_click` (Rc handler) is skipped. The style-driving props (tone/variant/
// size/disabled/selected/size_px/radius/color) route to the `make_style`
// sink reading `.get()` INSIDE; `glyph`/`icon`/`size_px`/`size` route to the
// child sink. A bare value stays a zero-cost `Static` snapshot (the
// no-flicker fast path); a `Signal`/`rx!` re-styles in place.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct IconButtonProps {
    /// The single glyph/character rendered inside the square (e.g. `"×"`).
    /// Used only when `icon` is `None` — pass `icon` to render a vector
    /// (Lucide) icon instead of a text glyph.
    pub glyph: String,
    /// Optional vector icon to render inside the square. When `Some`, it
    /// takes precedence over `glyph` — e.g. `icon = Some(icons_lucide::X)`
    /// for a Lucide close button. Inherits the button's tone text color.
    pub icon: Option<IconData>,
    /// Fires on press/click.
    pub on_click: Rc<dyn Fn()>,
    /// Semantic color palette (Neutral, Primary, Danger, …). Default Neutral.
    pub tone: ToneRef,
    /// Surface treatment (Filled, Ghost, Soft, …). Default Filled.
    pub variant: VariantRef,
    /// Square dimension preset (Sm, Md, Lg). Default Md.
    pub size: IconButtonSize,
    /// When `true`, blocks the press and dims the button. Default `false`.
    /// For reactive disabling, read a signal in the enclosing render scope
    /// (`disabled = some_state.get()`); the scope re-renders on change.
    pub disabled: bool,
    /// When `true`, paints the button with the tone's accent fill — the
    /// "active toggle" look (a selected tool in a toolbar). Default `false`.
    /// Pair with `variant = Ghost` for the transparent-resting tool button that
    /// fills with the accent when active. For reactive selection, read a signal
    /// in the enclosing render scope (`selected = tool.get() == Tool::Pen`).
    pub selected: bool,
    /// Override the square's side length, in px. When `Some`, it replaces the
    /// `size` step's width/height and rescales the icon — for fitting a specific
    /// layout (e.g. a 42px toolbar button) while still using the themed
    /// IconButton. Default `None` (the `size` enum decides).
    pub size_px: Option<f32>,
    /// Override the corner radius, in px. Default `None` uses the sheet's pill
    /// radius; set e.g. `12.0` for a rounded-square tool button.
    pub radius: Option<f32>,
    /// Override the icon/glyph color. Default `None` uses the tone × variant
    /// foreground. Useful when the resting color should be neutral (ink) while
    /// the `selected` accent fill still supplies its own contrasting text — pass
    /// the ink color only on the un-selected instance.
    pub color: Option<Color>,
}

impl Default for IconButtonProps {
    fn default() -> Self {
        Self {
            glyph: Reactive::Static(String::new()),
            icon: Reactive::Static(None),
            on_click: Rc::new(|| {}),
            tone: tone::Neutral.into(),
            variant: variant::Filled.into(),
            size: Reactive::Static(IconButtonSize::default()),
            disabled: Reactive::Static(false),
            selected: Reactive::Static(false),
            size_px: Reactive::Static(None),
            radius: Reactive::Static(None),
            color: Reactive::Static(None),
        }
    }
}

/// Renders a square, single-glyph clickable styled by the tone × variant
/// × size axes of the installed IconButton sheet.
#[component]
pub fn IconButton(props: &IconButtonProps) -> Element {
    let on_click = props.on_click.clone();

    // TODO(reactive-sweep): route the child-structure props (`glyph`, `icon`,
    // and the `size`/`size_px`-derived icon pixel size) reactively. Which child
    // is rendered (vector `icon` vs text `glyph`) and the icon's pinned square
    // are STRUCTURAL — switching them on a live signal needs a `switch`/`when`
    // wrapper, not an in-place sink. These are snapshotted at build; a live
    // `glyph`/`icon`/`size`/`size_px` won't swap the child in place.
    let icon_data = props.icon.get();
    let glyph = props.glyph.get();
    let size_snapshot = props.size.get();
    let size_px_snapshot = props.size_px.get();

    // The style is REACTIVE when any style-driving prop is live; otherwise it
    // stays the build-time fast path (applied before first paint, theme-swapped
    // in bulk — no per-node Effect, no first-paint flicker). The closure reads
    // each prop live INSIDE, so the apply-style Effect subscribes to whichever
    // are dynamic. `selected` drives the accent-fill toggle overlay (the active
    // tool-button look).
    let style_is_reactive = !props.tone.is_static()
        || !props.variant.is_static()
        || !props.size.is_static()
        || !props.selected.is_static()
        || !props.size_px.is_static()
        || !props.radius.is_static()
        || !props.color.is_static();

    let make_style = {
        let tone = props.tone.clone();
        let variant = props.variant.clone();
        let size = props.size.clone();
        let selected = props.selected.clone();
        let size_px = props.size_px.clone();
        let radius = props.radius.clone();
        let color = props.color.clone();
        move || -> StyleApplication {
            let appearance_key = format!("{}_{}", tone.get().key(), variant.get().key());
            let size_key = size.get().as_variant_str().to_string();

            let mut style = StyleApplication::new(installed_icon_button_sheet())
                .with("appearance", appearance_key)
                .with("size", size_key)
                .with("selected", if selected.get() { "on" } else { "off" });

            // One computed layer (the StyleApplication has a single slot): hug +
            // center on the cross axis, plus an optional custom square size.
            // Without `align_self: Center` a flex parent's default `align-items:
            // stretch` top-aligns the square in a row of mixed sizes (the
            // IconButton "Sizes" row). Keyed by `size_px` so distinct sizes
            // don't share a cache entry.
            let sp = size_px.get();
            let layer_key =
                format!("ib-layout-{}", sp.map(|p| (p * 100.0).round() as i32).unwrap_or(-1));
            style = style.with_computed(layer_key, move || {
                let mut rules =
                    StyleRules { align_self: Some(AlignSelf::Center), ..Default::default() };
                if let Some(px) = sp {
                    rules.width = Some(Tokenized::Literal(Length::Px(px)));
                    rules.height = Some(Tokenized::Literal(Length::Px(px)));
                }
                rules
            });
            if let Some(r) = radius.get() {
                style = style.override_border_radius(Length::Px(r));
            }
            if let Some(c) = color.get() {
                style = style.override_color(Tokenized::Literal(c));
            }
            style
        }
    };

    // A vector `icon` wins over the text `glyph`. The icon inherits the
    // button's tone text color (the primitive defaults to the ambient
    // label color), so it tints correctly per variant without an
    // explicit color. Sized to the square's content box per size step.
    // Icon scales with the square: half the side for a custom `size_px`,
    // otherwise the per-step default.
    let icon_px = size_px_snapshot
        .map(|s| (s * 0.5).round())
        .unwrap_or_else(|| icon_px_for(size_snapshot));
    let child = match icon_data {
        Some(data) => icon(data)
            .with_style(icon_button_icon_sheet(icon_px))
            .into_element(),
        None => text(glyph).into_element(),
    };
    let mut bound = runtime_core::pressable(vec![child], move || (on_click)());
    bound = if style_is_reactive {
        bound.with_style(make_style)
    } else {
        bound.with_style(make_style())
    };

    // `disabled` routes reactively: `.disabled()` accepts a `Fn() -> bool`, so
    // reading `.get()` inside subscribes the disabled state to a live signal
    // (it dims + blocks the press in place). A static value reads once.
    if !props.disabled.is_static() {
        let disabled = props.disabled.clone();
        bound = bound.disabled(move || disabled.get());
    } else if props.disabled.get() {
        bound = bound.disabled(true);
    }
    bound.into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::FillRule;

    fn theme() {
        install_idea_theme(light_theme());
    }

    const TRASH: IconData = IconData {
        view_box: (24, 24),
        paths: &["M3 6h18M8 6V4h8v2M19 6l-1 14H6L5 6"],
        fill_rule: FillRule::NonZero,
        filled: false,
    };

    fn only_child(el: Element) -> Element {
        match el {
            Element::Pressable { mut children, .. } => {
                assert_eq!(children.len(), 1, "IconButton has a single child");
                children.remove(0)
            }
            _ => panic!("IconButton renders a Pressable"),
        }
    }

    // D5: passing `icon` (IconData) renders a vector icon instead of the
    // text glyph — so Lucide SVGs work where only a String glyph did.
    #[test]
    fn icon_data_renders_an_icon_child() {
        theme();
        let props = IconButtonProps {
            icon: Reactive::Static(Some(TRASH)),
            glyph: Reactive::Static("×".into()), // present but overridden by `icon`
            ..Default::default()
        };
        assert!(
            matches!(only_child(IconButton(&props)), Element::Icon { .. }),
            "an `icon` prop must render an Icon child (it wins over glyph)"
        );
    }

    // The glyph path still works when no `icon` is given.
    #[test]
    fn glyph_falls_back_to_text() {
        theme();
        let props = IconButtonProps {
            glyph: Reactive::Static("×".into()),
            ..Default::default()
        };
        assert!(
            matches!(only_child(IconButton(&props)), Element::Text { .. }),
            "with no `icon`, the glyph renders as text"
        );
    }

    #[test]
    fn icon_size_scales_with_button_size() {
        assert_eq!(icon_px_for(IconButtonSize::Sm), 16.0);
        assert_eq!(icon_px_for(IconButtonSize::Md), 18.0);
        assert_eq!(icon_px_for(IconButtonSize::Lg), 24.0);
    }

    // Regression: an IconButton must center on its parent's cross axis
    // (`align_self: Center`) so a row of mixed-size icon buttons centers
    // instead of top-aligning under the parent's default `align-items:
    // stretch` (the IconButton "Sizes" row report).
    #[test]
    fn icon_button_centers_on_cross_axis() {
        theme();
        let app = match IconButton(&IconButtonProps::default()) {
            Element::Pressable { style: Some(runtime_core::StyleSource::Static(a)), .. } => a,
            _ => panic!("IconButton renders a statically-styled Pressable"),
        };
        assert_eq!(
            runtime_core::resolve_style(&app).align_self,
            Some(AlignSelf::Center),
            "an IconButton centers on the cross axis instead of stretching/top-aligning"
        );
    }
}
