//! `Stack` — an opinionated flex container.
//!
//! Wraps a `View` with the [`Stack`](crate::stylesheets::Stack) stylesheet
//! pre-applied. One component covers both column and row layouts via the
//! `axis` prop; the default is `Column` (the common case for screens and
//! card bodies). Use `axis = StackAxis::Row` for row layouts (toolbars,
//! button groups, badge rows).

use runtime_core::{component, derived, ui, ChildList, Element, IdealystSchema, Reactive};

use crate::stylesheets::Stack as StackStyle;
use crate::stylesheets::StackWrap;

// Re-export the stylesheet-generated variant enums.
pub use crate::stylesheets::{StackAlign, StackAxis, StackGap, StackJustify, StackPadding};

// Reactive-by-default: `#[props]` wraps each scalar-DATA field → `Reactive<…>`
// (`gap`/`padding`/`axis`/`align`/`justify`/`wrap`). All drive the Stack
// stylesheet, so they route into the style sink reading `.get()` live;
// `children` is the children category and stays bare. A bare value stays a
// zero-cost `Static` snapshot (keeps the build-time `StyleSource::Static` fast
// path); a `Signal`/`rx!` re-styles in place.
#[runtime_core::props]
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
    // Route each prop into the stylesheet's own variant sink (mirrors
    // `Divider`): a `Static` value takes the build-time fast path, a live one is
    // passed via `derived(...)` so the generated builder emits
    // `StyleSource::Reactive` and re-resolves that variant on change. Reading
    // `prop.get()` INSIDE the `derived` closure is what subscribes the
    // apply-style Effect to the source.
    let mut style = StackStyle();
    style = match props.gap.clone() {
        Reactive::Static(v) => style.gap(v),
        dynamic => style.gap(derived(move || dynamic.get())),
    };
    style = match props.padding.clone() {
        Reactive::Static(v) => style.padding(v),
        dynamic => style.padding(derived(move || dynamic.get())),
    };
    style = match props.axis.clone() {
        Reactive::Static(v) => style.axis(v),
        dynamic => style.axis(derived(move || dynamic.get())),
    };
    style = match props.align.clone() {
        Reactive::Static(v) => style.align(v),
        dynamic => style.align(derived(move || dynamic.get())),
    };
    style = match props.justify.clone() {
        Reactive::Static(v) => style.justify(v),
        dynamic => style.justify(derived(move || dynamic.get())),
    };
    style = match props.wrap.clone() {
        Reactive::Static(v) => {
            style.wrap(if v { StackWrap::On } else { StackWrap::Off })
        }
        dynamic => style.wrap(derived(move || {
            if dynamic.get() { StackWrap::On } else { StackWrap::Off }
        })),
    };

    let mut children: Vec<Element> = Vec::with_capacity(props.children.len());
    for c in props.children {
        ChildList::append_to(c, &mut children);
    }
    ui! { view(style = style) { children } }
}

#[cfg(test)]
mod tests {
    use super::*;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, AlignItems, StyleSource};

    // A row Stack with `align = Center` (alongside axis/gap/justify) must
    // resolve to `align_items: Center`. Guards that chaining the `align`
    // variant alongside the others isn't dropped (the icon-Sizes-row report,
    // which turned out to be a stale build — this pins the resolution).
    #[test]
    fn align_center_resolves_to_align_items_center() {
        install_idea_theme(light_theme());
        let el = Stack(StackProps {
            axis: Reactive::Static(StackAxis::Row),
            gap: Reactive::Static(StackGap::Lg),
            align: Reactive::Static(StackAlign::Center),
            justify: Reactive::Static(StackJustify::Start),
            ..Default::default()
        });
        let app = match el {
            Element::View { style: Some(StyleSource::Static(a)), .. } => a,
            _ => panic!("Stack renders a statically-styled View"),
        };
        assert_eq!(
            resolve_style(&app).align_items,
            Some(AlignItems::Center),
            "align = Center must resolve to align_items: Center even with axis/gap/justify set"
        );
    }
}
