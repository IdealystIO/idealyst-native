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
    component, icon, text, Element, IconData, IdealystSchema, IntoElement, Length, StyleApplication,
    StyleRules, StyleSheet, Tokenized, VariantEnum,
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
}

impl Default for IconButtonProps {
    fn default() -> Self {
        Self {
            glyph: String::new(),
            icon: None,
            on_click: Rc::new(|| {}),
            tone: tone::Neutral.into(),
            variant: variant::Filled.into(),
            size: IconButtonSize::default(),
            disabled: false,
        }
    }
}

/// Renders a square, single-glyph clickable styled by the tone × variant
/// × size axes of the installed IconButton sheet.
#[component]
pub fn IconButton(props: &IconButtonProps) -> Element {
    let glyph = props.glyph.clone();
    let icon_data = props.icon;
    let on_click = props.on_click.clone();
    let tone = props.tone.clone();
    let variant = props.variant.clone();
    let size = props.size;
    let disabled = props.disabled;

    let appearance_key = format!("{}_{}", tone.key(), variant.key());
    let size_key = size.as_variant_str().to_string();

    // Static style — build-time apply, no flicker (see Button).
    let style = StyleApplication::new(installed_icon_button_sheet())
        .with("appearance", appearance_key)
        .with("size", size_key);

    // A vector `icon` wins over the text `glyph`. The icon inherits the
    // button's tone text color (the primitive defaults to the ambient
    // label color), so it tints correctly per variant without an
    // explicit color. Sized to the square's content box per size step.
    let child = match icon_data {
        Some(data) => icon(data)
            .with_style(icon_button_icon_sheet(icon_px_for(size)))
            .into_element(),
        None => text(glyph).into_element(),
    };
    let mut bound = runtime_core::pressable(vec![child], move || (on_click)())
        .with_style(style);
    if disabled {
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
            icon: Some(TRASH),
            glyph: "×".into(), // present but overridden by `icon`
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
            glyph: "×".into(),
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
}
