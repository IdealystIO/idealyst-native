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

use runtime_core::{
    component, icon, Color, Element, IconData, IdealystSchema, IntoElement, Reactive,
};

use idea_theme::extensible::ToneRef;
use idea_theme::theme::IdeaThemeRef;

/// Default rendered size (square, in points) when `size` is left at its
/// default. Matches the body text cap-height region so an inline icon
/// sits comfortably beside a label.
pub const ICON_DEFAULT_PX: f32 = 20.0;

// Reactive-by-default: `#[props]` wraps each data field `T` → `Reactive<T>`.
// `tone`/`color` route to the primitive's reactive `.color()` closure (read
// `.get()` inside, so the override re-tints on a live tone/color). `data` and
// `size` are snapshotted: the `icon` primitive has no reactive `data`/`size`
// setter (`data` is fixed at construction, `.size(f32)` applies a static
// sheet) — see the TODO in the body.
#[runtime_core::props]
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
            data: Reactive::Static(EMPTY_ICON),
            size: Reactive::Static(ICON_DEFAULT_PX),
            tone: Reactive::Static(None),
            color: Reactive::Static(None),
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

/// Renders a sized, optionally tinted vector icon. Wraps the framework's
/// `icon` primitive so call sites get a themed `#[component]` instead of
/// the raw primitive.
#[component]
pub fn Icon(props: &IconProps) -> Element {
    // `size` is a `.size()` sizing-sheet pin (a plain `f32`); a live `size`
    // is snapshotted here. TODO(reactive-sweep): a reactive `.size()` setter
    // on `Bound<IconHandle>` would route it (same shape as `.data()` below).
    let size = props.size.get();
    let tone = props.tone.clone();
    let explicit = props.color.clone();

    // `data` is routed LIVE: a reactive source swaps the rendered glyph in
    // place via the primitive's reactive `.data()` setter (no node rebuild).
    // A `Static` data installs no effect (the create-time glyph).
    let mut node = icon(props.data.get()).size(size);
    if !props.data.is_static() {
        let data = props.data.clone();
        node = node.data(move || data.get());
    }

    // `tone`/`color` are routed to the primitive's reactive `.color()` closure.
    // Reading `.get()` INSIDE the closure subscribes the icon's color Effect to
    // a live tone/color, so it re-tints in place (and the tone path already
    // re-tints on a theme swap via `Tokenized::resolve()`). A static tone/color
    // resolves once. Tone wins over the explicit color (matches the snapshot
    // precedence below).
    let has_tone = matches!(tone.get(), Some(_));
    let has_color = matches!(explicit.get(), Some(_));
    if has_tone {
        node = node.color(move || {
            let theme_rc = idea_theme::active_theme();
            let theme_ref = theme_rc
                .downcast_ref::<IdeaThemeRef>()
                .expect("idea-ui: no IdeaTheme installed");
            // Tone is live here: read it inside so a reactive tone re-resolves.
            let t = tone.get().expect("tone present (checked above)");
            t.ghost_fg(theme_ref).resolve()
        });
    } else if has_color {
        node = node.color(move || explicit.get().expect("color present (checked above)"));
    }

    node.into_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::extensible::tone;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{
        resolve_style, FillRule, Length, StyleApplication, StyleSource, Tokenized,
    };

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
            data: Reactive::Static(DOT),
            tone: Reactive::Static(Some(tone::Primary.into())),
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
        let props = IconProps { data: Reactive::Static(DOT), ..Default::default() };
        let (has_color, _) = icon_parts(Icon(&props));
        assert!(!has_color, "an untinted Icon leaves color to inherit");
    }

    // D5: `size` pins an explicit square so the icon doesn't collapse to
    // 0×0 under flex.
    #[test]
    fn size_pins_an_explicit_square() {
        theme();
        let props = IconProps {
            data: Reactive::Static(DOT),
            size: Reactive::Static(28.0),
            ..Default::default()
        };
        let (_, app) = icon_parts(Icon(&props));
        let rules = resolve_style(&app);
        assert_eq!(rules.width, Some(Tokenized::Literal(Length::Px(28.0))));
        assert_eq!(rules.height, Some(Tokenized::Literal(Length::Px(28.0))));
    }
}
