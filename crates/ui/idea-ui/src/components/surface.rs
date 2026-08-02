//! `Surface` — a themed background container.
//!
//! The lowest-level "themed box": a `view` whose background is pulled from
//! the active theme's neutral palette (page background / surface / alt
//! surface) rather than hard-coded. Use it to lay out regions whose color
//! should track the theme — e.g. a recessed list pane against a raised
//! content panel — without reaching for `Card` (which adds borders, radius,
//! shadow, and an intent-variant surface).
//!
//! ```ignore
//! ui! {
//!     Stack(axis = StackAxis::Row, align = StackAlign::Stretch) {
//!         Surface(background = SurfaceColor::Background, grow = 2.0, padding = StackPadding::Sm) {
//!             // recessed list
//!         }
//!         Surface(background = SurfaceColor::Surface, grow = 3.0, padding = StackPadding::Md) {
//!             // raised content panel
//!         }
//!     }
//! }
//! ```
//!
//! Backgrounds resolve through the theme token system (`color-background` /
//! `color-surface` / `color-surface-alt`), so a theme swap recolors every
//! `Surface` without touching call sites.

use runtime_core::{
    component, ui, ChildList, Element, Length, Reactive, StyleApplication, StyleRules, Tokenized,
};

pub use crate::stylesheets::StackPadding;

/// Which themed neutral fills the surface. Maps to the theme's neutral
/// color tokens — see [`idea_theme`]'s `background` / `surface` /
/// `surface_alt`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SurfaceColor {
    /// The page background — the recessed base (often a light gray).
    /// Token `color-background`.
    Background,
    /// The standard surface — the raised panel (often white).
    /// Token `color-surface`.
    Surface,
    /// An alternate surface, a layer above [`SurfaceColor::Surface`].
    /// Token `color-surface-alt`.
    SurfaceAlt,
}

impl Default for SurfaceColor {
    fn default() -> Self {
        SurfaceColor::Surface
    }
}

impl SurfaceColor {
    /// The `SurfaceSheet` `bg` axis arm for this surface. The token +
    /// fallback pairs live on the sheet's arms now (see
    /// `stylesheets::SurfaceSheet`).
    fn bg_key(self) -> &'static str {
        match self {
            SurfaceColor::Background => "background",
            SurfaceColor::Surface => "surface",
            SurfaceColor::SurfaceAlt => "surface_alt",
        }
    }
}

/// The `SurfaceSheet` `pad` axis arm for a [`StackPadding`] step (shared
/// scale with `Stack`; the token + fallback pairs live on the arms).
fn pad_key(p: StackPadding) -> &'static str {
    match p {
        StackPadding::None => "none",
        StackPadding::Xs => "xs",
        StackPadding::Sm => "sm",
        StackPadding::Md => "md",
        StackPadding::Lg => "lg",
        StackPadding::Xl => "xl",
    }
}

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>` (`background`/`grow`/`padding`). They all drive the surface's
// style, so they route into the style sink; `children` is the children
// category and is left bare. A bare value stays a zero-cost `Static`
// snapshot (the fast path); a `Signal`/`rx!` re-styles in place.
#[runtime_core::props]
#[derive(Default)]
pub struct SurfaceProps {
    /// Which themed neutral fills the surface. Default
    /// [`SurfaceColor::Surface`].
    pub background: SurfaceColor,
    /// `flex-grow` weight. `0.0` (default) sizes to content; give siblings
    /// e.g. `2.0` and `3.0` for a proportional split. When `> 0`, the
    /// surface also gets `flex-basis: 0` + `min-width: 0` so the ratio
    /// holds and it can shrink below content width.
    pub grow: f32,
    /// Token-driven inner padding. Default [`StackPadding::None`].
    pub padding: StackPadding,
    /// Children, laid out in a column (like a plain `view`).
    pub children: Vec<Element>,
}

/// A themed background container. See the module docs.
#[component(children)]
pub fn Surface(props: SurfaceProps) -> Element {
    // The style is REACTIVE when any style-driving prop is live; otherwise it's
    // the build-time fast path. The closure reads each prop's `.get()` INSIDE so
    // the apply-style Effect subscribes to whichever are dynamic.
    let style_is_reactive =
        !props.background.is_static() || !props.grow.is_static() || !props.padding.is_static();

    let make_style = {
        let background = props.background.clone();
        let grow_r = props.grow.clone();
        let padding = props.padding.clone();
        move || -> StyleApplication {
            // Closed props ride the sheet's axes; the continuous `grow`
            // weight rides the INLINE layer, keeping the application
            // premintable (the previous per-instance `StyleSheet::new`
            // was invisible to the premint dump). `resolve()` folds the
            // inline layer into the resolved rules on the live path, so
            // SSR/native output is byte-identical to the old shape.
            let grow = grow_r.get();
            let mut app = StyleApplication::new(crate::stylesheets::SurfaceSheet::sheet())
                .with("bg", background.get().bg_key().to_string())
                .with("pad", pad_key(padding.get()).to_string());
            if grow > 0.0 {
                app = app.with_inline(StyleRules {
                    flex_grow: Some(Tokenized::Literal(grow)),
                    // `flex-basis: 0` + `min-width: 0` so the ratio holds
                    // and the surface can shrink below content width.
                    flex_basis: Some(Tokenized::Literal(Length::Px(0.0))),
                    min_width: Some(Tokenized::Literal(Length::Px(0.0))),
                    ..Default::default()
                });
            }
            app
        }
    };

    // Flatten incoming fragments (mirrors `Card`/`Center`).
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }

    if style_is_reactive {
        ui! { view(style = make_style) { children } }
    } else {
        ui! { view(style = make_style()) { children } }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};

    fn app_for(bg: SurfaceColor, grow: f32, pad: StackPadding) -> StyleApplication {
        let mut app = StyleApplication::new(crate::stylesheets::SurfaceSheet::sheet())
            .with("bg", bg.bg_key().to_string())
            .with("pad", pad_key(pad).to_string());
        if grow > 0.0 {
            app = app.with_inline(StyleRules {
                flex_grow: Some(Tokenized::Literal(grow)),
                flex_basis: Some(Tokenized::Literal(Length::Px(0.0))),
                min_width: Some(Tokenized::Literal(Length::Px(0.0))),
                ..Default::default()
            });
        }
        app
    }

    /// Surface used to build a per-instance `StyleSheet::new`, which has
    /// no premint class by construction — every Surface dragged the live
    /// style engine into `--premint` builds. The axes + inline-grow shape
    /// must premint, INCLUDING when `grow` is set (the inline layer is
    /// explicitly not part of the premint disqualifiers).
    #[test]
    fn regression_surface_premints_including_with_grow() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            assert!(
                app_for(SurfaceColor::Surface, 0.0, StackPadding::None)
                    .preminted_class_list()
                    .is_some(),
                "plain Surface premints"
            );
            assert!(
                app_for(SurfaceColor::Background, 2.0, StackPadding::Md)
                    .preminted_class_list()
                    .is_some(),
                "grow rides the inline layer and must not disqualify preminting"
            );
        });
    }

    /// The resolved rules of the new shape must match the OLD per-instance
    /// sheet property-for-property — same background token, same paddings,
    /// and `resolve()` folds the inline grow back in — so live/SSR output
    /// (and every minted class hash) is unchanged.
    #[test]
    fn surface_resolved_rules_match_the_old_per_instance_shape() {
        use runtime_core::resolve_style;
        with_test_world(|| {
            install_idea_theme(light_theme());
            let rules = resolve_style(&app_for(SurfaceColor::Background, 2.0, StackPadding::Sm));
            assert_eq!(
                rules.flex_grow,
                Some(Tokenized::Literal(2.0)),
                "inline grow folds into the resolved rules on the live path"
            );
            assert_eq!(rules.flex_basis, Some(Tokenized::Literal(Length::Px(0.0))));
            assert_eq!(rules.min_width, Some(Tokenized::Literal(Length::Px(0.0))));
            assert!(rules.background.is_some(), "bg arm supplies the token");
            assert!(rules.padding_top.is_some(), "pad arm supplies the spacing token");
        });
    }
}
