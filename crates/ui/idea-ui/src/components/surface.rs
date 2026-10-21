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
    component, component_styles, ui, AlignItems, ChildList, Element, FlexDirection, Length, Reactive, StyleRules,
    Tokenized,
};

use crate::components::stack::StackPadding;
use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Surface`].
    pub Surface<IdeaThemeRef> {
        /// Which themed neutral fills the surface.
        axis color {
            /// The page background — the recessed base. Token `color-background`.
            background,
            /// The standard surface — the raised panel. Token `color-surface`.
            #[default]
            surface,
            /// A layer above `Surface`. Token `color-surface-alt`.
            surface_alt,
        }
        /// The container: which neutral token fills it, which spacing step
        /// pads it. The continuous `grow` weight rides the inline layer, so
        /// the slot premints whatever the weight.
        slot root {
            base(t) {
                flex_direction: FlexDirection::Column,
                // Children fill the cross axis so inner content spans the surface.
                align_items: AlignItems::Stretch,
            }
            variant color {
                background(t) { background: t.color.background() }
                #[default]
                surface(t) { background: t.color.surface() }
                surface_alt(t) { background: t.color.surface_alt() }
            }
            variant padding {
                #[default]
                none(_t) {}
                xs(t) { padding: t.spacing.xs() }
                sm(t) { padding: t.spacing.sm() }
                md(t) { padding: t.spacing.md() }
                lg(t) { padding: t.spacing.lg() }
                xl(t) { padding: t.spacing.xl() }
            }
        }
    }
}

/// The `Stack` spacing scale converts into Surface's own padding axis, so
/// `padding = StackPadding::Md` reads the same on both.
impl From<StackPadding> for SurfacePadding {
    fn from(p: StackPadding) -> Self {
        // Same arm names on both axes.
        SurfacePadding::define(p.key())
    }
}

impl From<StackPadding> for Reactive<SurfacePadding> {
    fn from(p: StackPadding) -> Self {
        Reactive::Static(p.into())
    }
}

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>` (`background`/`grow`/`padding`). They all drive the surface's
// style, so they route into the style sink; `children` is the children
// category and is left bare. A bare value stays a zero-cost `Static`
// snapshot (the fast path); a `Signal`/`rx!` re-styles in place.
#[runtime_core::props(styles = SurfaceStyles)]
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
    /// Token-driven inner padding — Surface's own axis; a `StackPadding`
    /// converts. Default `None`.
    pub padding: SurfacePadding,
    /// Children, laid out in a column (like a plain `view`).
    pub children: Vec<Element>,
}

/// A themed background container. See the module docs.
#[component(children)]
pub fn Surface(props: SurfaceProps) -> Element {
    // The closed props ride the slot's axes; the continuous `grow` weight
    // rides the INLINE layer, keeping the slot premintable (a per-instance
    // sheet would be invisible to the premint dump).
    let grow_rules = |grow: f32| {
        if grow > 0.0 {
            StyleRules {
                flex_grow: Some(Tokenized::Literal(grow)),
                // `flex-basis: 0` + `min-width: 0` so the ratio holds and
                // the surface can shrink below content width.
                flex_basis: Some(Tokenized::Literal(Length::Px(0.0))),
                min_width: Some(Tokenized::Literal(Length::Px(0.0))),
                ..Default::default()
            }
        } else {
            StyleRules::default()
        }
    };
    let style = SurfaceStyles::root().set(props.background.clone()).set(props.padding.clone());
    let style = match props.grow.clone() {
        Reactive::Static(g) if g > 0.0 => style.inline(grow_rules(g)),
        Reactive::Static(_) => style,
        live => style.inline_with(move || grow_rules(live.get())),
    };
    let style = style.overridden(props.styles.root.as_ref());

    // Flatten incoming fragments (mirrors `Card`/`Center`).
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }

    ui! { view(style = style) { children } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};

    fn app_for(bg: SurfaceColor, grow: f32, pad: StackPadding) -> runtime_core::StyleApplication {
        let el = Surface(SurfaceProps {
            background: Reactive::Static(bg),
            grow: Reactive::Static(grow),
            padding: pad.into(),
            ..Default::default()
        });
        match crate::test_support::classify(el) {
            crate::test_support::P::View { style, .. } => style.expect("styled").application(),
            _ => panic!("Surface renders a View"),
        }
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

    /// The resolved rules carry the background token, the paddings, and
    /// the inline grow (with its `flex_basis`/`min_width` zeroes) folded
    /// back in by `resolve()` on the live path.
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
