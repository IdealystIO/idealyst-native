//! `Icon` — a themed wrapper around the `icon` primitive.
//!
//! ```ignore
//! use idea_ui::{tone, Icon};
//! use icons_lucide::SEARCH;
//!
//! ui! {
//!     Icon(data = SEARCH, size = 20.0, tone = Some(tone::Primary.into()))
//! }
//! ```
//!
//! The raw `icon(data)` primitive renders vector data but has no
//! intrinsic size and inherits its color from the surrounding text.
//! `Icon` pins an explicit square `size` and lets call sites tint it
//! either by a semantic `tone` (resolved to the tone's intent color,
//! theme-reactive) or an explicit `color`. With neither set, the icon
//! inherits the ambient text color — matching the primitive's default.

use std::rc::Rc;

use runtime_core::{
    component, icon, Color, Element, IconData, IdealystSchema, IntoElement, Length, StyleRules,
    StyleSheet, Tokenized,
};

use idea_theme::extensible::ToneRef;
use idea_theme::theme::IdeaThemeRef;

/// Default rendered size (square, in points) when `size` is left at its
/// default. Matches the body text cap-height region so an inline icon
/// sits comfortably beside a label.
pub const ICON_DEFAULT_PX: f32 = 20.0;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct IconProps {
    /// The vector icon to render. Pass an `IconData` constant from an
    /// icon pack (e.g. `icons_lucide::SEARCH`). Only the constants you
    /// reference end up in the binary.
    pub data: IconData,
    /// Rendered square size in points. Default [`ICON_DEFAULT_PX`].
    pub size: f32,
    /// Optional semantic tint. When `Some`, the icon paints in the
    /// tone's intent color (theme-reactive — it re-tints on theme swap).
    /// Takes precedence over `color`.
    pub tone: Option<ToneRef>,
    /// Optional explicit color override. Used only when `tone` is `None`.
    /// When both are `None`, the icon inherits the ambient text color.
    pub color: Option<Color>,
}

impl Default for IconProps {
    fn default() -> Self {
        Self {
            data: EMPTY_ICON,
            size: ICON_DEFAULT_PX,
            tone: None,
            color: None,
        }
    }
}

/// A zero-path placeholder so `IconProps` can derive a `Default` (the
/// `#[component]` dispatch requires it). A real call site always passes
/// `data`; this never renders anything visible.
const EMPTY_ICON: IconData = IconData {
    view_box: (24, 24),
    paths: &[],
    fill_rule: runtime_core::FillRule::NonZero,
    filled: false,
};

thread_local! {
    // Keyed by integer-encoded size (px * 100, rounded) so distinct
    // sizes get distinct cached sheets without float-key hashing.
    static ICON_SIZE_SHEETS: std::cell::RefCell<
        std::collections::HashMap<u32, Rc<StyleSheet>>,
    > = std::cell::RefCell::new(std::collections::HashMap::new());
}

/// A cached static sheet pinning the icon to a `px × px` square. Icons
/// have no intrinsic content size, so an explicit width/height keeps
/// them from collapsing to a 0×0 box under flex.
fn icon_size_sheet(px: f32) -> Rc<StyleSheet> {
    let key = (px * 100.0).round() as u32;
    ICON_SIZE_SHEETS.with(|m| {
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

/// Renders a sized, optionally tinted vector icon. Wraps the framework's
/// `icon` primitive so call sites get a themed `#[component]` instead of
/// the raw primitive.
#[component]
pub fn Icon(props: &IconProps) -> Element {
    let data = props.data;
    let size = props.size;
    let tone = props.tone.clone();
    let explicit = props.color.clone();

    let mut node = icon(data).with_style(icon_size_sheet(size));

    if let Some(tone) = tone {
        // Tone wins: resolve the tone's intent (ghost) color through the
        // active theme. `Tokenized::resolve()` subscribes to the token,
        // so the icon re-tints reactively on a theme swap.
        node = node.color(move || {
            let theme_rc = idea_theme::active_theme();
            let theme_ref = theme_rc
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            tone.ghost_fg(theme_ref).resolve()
        });
    } else if let Some(c) = explicit {
        node = node.color(move || c.clone());
    }

    node.into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::extensible::tone;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, FillRule, StyleApplication, StyleSource};

    fn theme() {
        install_idea_theme(light_theme());
    }

    const DOT: IconData = IconData {
        view_box: (24, 24),
        paths: &["M12 12h.01"],
        fill_rule: FillRule::NonZero,
        filled: true,
    };

    fn icon_parts(el: Element) -> (bool, StyleApplication) {
        match el {
            Element::Icon { color, style, .. } => {
                let app = match style.expect("Icon always pins a size style") {
                    StyleSource::Static(a) => a,
                    _ => panic!("Icon uses a static size sheet"),
                };
                (color.is_some(), app)
            }
            _ => panic!("Icon renders an Icon primitive"),
        }
    }

    // D5: a tone tints the icon — the primitive's `color` override is set.
    #[test]
    fn tone_sets_a_color_override() {
        theme();
        let props = IconProps {
            data: DOT,
            tone: Some(tone::Primary.into()),
            ..Default::default()
        };
        let (has_color, _) = icon_parts(Icon(&props));
        assert!(has_color, "a toned Icon installs a color override");
    }

    // With neither tone nor color, the icon inherits ambient text color
    // (no override) — matching the raw primitive's default.
    #[test]
    fn no_tint_inherits_ambient_color() {
        theme();
        let props = IconProps { data: DOT, ..Default::default() };
        let (has_color, _) = icon_parts(Icon(&props));
        assert!(!has_color, "an untinted Icon leaves color to inherit");
    }

    // D5: `size` pins an explicit square so the icon doesn't collapse to
    // 0×0 under flex.
    #[test]
    fn size_pins_an_explicit_square() {
        theme();
        let props = IconProps { data: DOT, size: 28.0, ..Default::default() };
        let (_, app) = icon_parts(Icon(&props));
        let rules = resolve_style(&app);
        assert_eq!(rules.width, Some(Tokenized::Literal(Length::Px(28.0))));
        assert_eq!(rules.height, Some(Tokenized::Literal(Length::Px(28.0))));
    }
}
