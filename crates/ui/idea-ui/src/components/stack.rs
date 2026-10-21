//! `Stack` — an opinionated flex container.
//!
//! A `View` with [`StackStyles`]' `root` slot applied. One component covers
//! both column and row layouts via the `axis` prop; the default is `Column`
//! (the common case for screens and card bodies). Use `axis = StackAxis::Row`
//! for row layouts (toolbars, button groups, badge rows).

use runtime_core::{component, component_styles, ui, AlignItems, ChildList, Element, FlexDirection, IdealystSchema, JustifyContent, Length};

use crate::theme::IdeaThemeRef;

component_styles! {
    /// The styled sections of [`Stack`].
    pub Stack<IdeaThemeRef> {
        slot root {
            base(t) {
                flex_direction: FlexDirection::Column,
                gap: t.spacing.md(),
            }
            variant gap {
                none(_t) { gap: Length::Px(0.0) }
                xs(t)    { gap: t.spacing.xs() }
                sm(t)    { gap: t.spacing.sm() }
                #[default]
                md(t)    { gap: t.spacing.md() }
                lg(t)    { gap: t.spacing.lg() }
                xl(t)    { gap: t.spacing.xl() }
            }
            variant padding {
                #[default]
                none(_t) { padding: Length::Px(0.0) }
                xs(t)    { padding: t.spacing.xs() }
                sm(t)    { padding: t.spacing.sm() }
                md(t)    { padding: t.spacing.md() }
                lg(t)    { padding: t.spacing.lg() }
                xl(t)    { padding: t.spacing.xl() }
            }
            variant axis {
                #[default]
                column(_t) { flex_direction: FlexDirection::Column }
                row(_t)    { flex_direction: FlexDirection::Row }
            }
            variant align {
                #[default]
                stretch(_t)  { align_items: AlignItems::Stretch }
                start(_t)    { align_items: AlignItems::FlexStart }
                center(_t)   { align_items: AlignItems::Center }
                end(_t)      { align_items: AlignItems::FlexEnd }
                // Align children on their text baseline — for inline rows that mix
                // prose and a Link/Badge so they sit on a common baseline.
                baseline(_t) { align_items: AlignItems::Baseline }
            }
            variant justify {
                #[default]
                start(_t)   { justify_content: JustifyContent::FlexStart }
                center(_t)  { justify_content: JustifyContent::Center }
                end(_t)     { justify_content: JustifyContent::FlexEnd }
                between(_t) { justify_content: JustifyContent::SpaceBetween }
                around(_t)  { justify_content: JustifyContent::SpaceAround }
            }
            // Opt-in line wrapping. `off` (default) keeps the row/column on a
            // single line (may overflow); `on` lets children wrap onto new
            // lines when they don't fit — the natural choice for a Row of
            // chips/buttons/badges on a narrow viewport.
            variant wrap {
                #[default]
                off(_t) { flex_wrap: runtime_core::FlexWrap::NoWrap }
                on(_t)  { flex_wrap: runtime_core::FlexWrap::Wrap }
            }
        }
    }
}

// Reactive-by-default: `#[props]` wraps each scalar-DATA field → `Reactive<…>`
// (`gap`/`padding`/`axis`/`align`/`justify`/`wrap`). All drive the Stack
// stylesheet, so they route into the style sink reading `.get()` live;
// `children` is the children category and stays bare. A bare value stays a
// zero-cost `Static` snapshot (keeps the build-time `StyleSource::Static` fast
// path); a `Signal`/`rx!` re-styles in place.
#[runtime_core::props(styles = StackStyles)]
#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct StackProps {
    /// Token-driven spacing between children. Tracks the theme's spacing
    /// scale. Default Md.
    pub gap: StackGap,
    /// Token-driven inner padding. Defaults to `None` so a Stack
    /// without an explicit `padding` prop matches its pre-padding
    /// behaviour. Sizes track the theme's spacing scale, same as
    /// `gap` — pick `Xs`/`Sm`/`Md`/`Lg`/`Xl` and the value comes
    /// from `t.spacing()` so it reflects the active theme.
    pub padding: StackPadding,
    /// Layout direction. Default Column.
    pub axis: StackAxis,
    /// Cross-axis alignment of children. Default Stretch.
    pub align: StackAlign,
    /// Main-axis distribution of children. Default Start.
    pub justify: StackJustify,
    /// Let children wrap onto new lines when they don't fit on one line.
    /// Default `false` (single line, may overflow). Set `true` for rows of
    /// chips/buttons/badges that should reflow on a narrow viewport instead
    /// of pushing the page wider than the screen.
    pub wrap: bool,
    /// The stacked children.
    pub children: Vec<Element>,
}

/// A flex container that lays out `children` along `axis` with token-
/// driven `gap`/`padding` and the chosen `align`/`justify`.
#[component(children)]
pub fn Stack(props: StackProps) -> Element {
    // Each prop routes into its own axis: a `Static` value takes the
    // build-time fast path, a live one re-resolves that axis on change.
    let style = StackStyles::root()
        .set(props.gap.clone())
        .set(props.padding.clone())
        .set(props.axis.clone())
        .set(props.align.clone())
        .set(props.justify.clone())
        .set(props.wrap.clone().map(StackWrap::from))
        .overridden(props.styles.root.as_ref());

    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { view(style = style) { children } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P, TStyle};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, AlignItems, Reactive};

    // A row Stack with `align = Center` (alongside axis/gap/justify) must
    // resolve to `align_items: Center`. Guards that chaining the `align`
    // variant alongside the others isn't dropped (the icon-Sizes-row report,
    // which turned out to be a stale build — this pins the resolution).
    #[test]
    fn align_center_resolves_to_align_items_center() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let el = Stack(StackProps {
                axis: Reactive::Static(StackAxis::Row),
                gap: Reactive::Static(StackGap::Lg),
                align: Reactive::Static(StackAlign::Center),
                justify: Reactive::Static(StackJustify::Start),
                ..Default::default()
            });
            let app = match classify(el) {
                P::View { style: Some(TStyle::App(a)), .. } => a,
                _ => panic!("Stack renders a statically-styled View"),
            };
            assert_eq!(
                resolve_style(&app).align_items,
                Some(AlignItems::Center),
                "align = Center must resolve to align_items: Center even with axis/gap/justify set"
            );
    });
    }
}
