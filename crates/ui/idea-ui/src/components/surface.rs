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

use std::rc::Rc;

use runtime_core::{
    component, ui, AlignItems, ChildList, Color, Element, FlexDirection, Length, StyleRules,
    StyleSheet, Tokenized, VariantSet,
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
    /// `(token-name, fallback-css-color)` for this surface.
    fn token(self) -> (&'static str, &'static str) {
        match self {
            SurfaceColor::Background => ("color-background", "#f3f4f6"),
            SurfaceColor::Surface => ("color-surface", "#ffffff"),
            SurfaceColor::SurfaceAlt => ("color-surface-alt", "#e5e7eb"),
        }
    }
}

/// Token name + fallback px for a [`StackPadding`] step (shared scale with
/// `Stack`). `None` → no padding.
fn padding_token(p: StackPadding) -> Option<Tokenized<Length>> {
    let (token, px) = match p {
        StackPadding::None => return None,
        StackPadding::Xs => ("spacing-xs", 4.0),
        StackPadding::Sm => ("spacing-sm", 8.0),
        StackPadding::Md => ("spacing-md", 12.0),
        StackPadding::Lg => ("spacing-lg", 16.0),
        StackPadding::Xl => ("spacing-xl", 24.0),
    };
    Some(Tokenized::token(token, Length::Px(px)))
}

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
    let (token, fallback) = props.background.token();
    let grow = props.grow;
    let pad = padding_token(props.padding);

    let style = Rc::new(StyleSheet::new(move |_vs: &VariantSet| StyleRules {
        background: Some(Tokenized::token(token, Color(fallback.to_string()))),
        flex_direction: Some(FlexDirection::Column),
        // Children fill the cross axis so inner content spans the surface.
        align_items: Some(AlignItems::Stretch),
        flex_grow: (grow > 0.0).then(|| Tokenized::Literal(grow)),
        flex_basis: (grow > 0.0).then(|| Tokenized::Literal(Length::Px(0.0))),
        min_width: (grow > 0.0).then(|| Tokenized::Literal(Length::Px(0.0))),
        padding_top: pad.clone(),
        padding_right: pad.clone(),
        padding_bottom: pad.clone(),
        padding_left: pad.clone(),
        ..Default::default()
    }));

    // Flatten incoming fragments (mirrors `Card`/`Center`).
    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }

    ui! { view(style = style) { children } }
}
