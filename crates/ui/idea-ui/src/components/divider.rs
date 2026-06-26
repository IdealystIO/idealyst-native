//! `Divider` — thin separator line, horizontal or vertical.

use runtime_core::{component, derived, ui, Element, IdealystSchema, Reactive};

use crate::stylesheets::Divider as DividerStyle;
pub use crate::stylesheets::DividerAxis;

// Reactive-by-default: `#[props]` wraps `axis` → `Reactive<DividerAxis>`. The
// derived `Default` still works (`Reactive::default()` = `Static(Default)`).
// `axis` routes into the style sink, read `.get()` INSIDE so the apply-style
// Effect subscribes when it's live; a bare value stays the static fast path.
#[runtime_core::props]
#[derive(Default, IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct DividerProps {
    /// Orientation of the separator line. Horizontal spans the parent's
    /// width (1px tall); Vertical spans its height (1px wide).
    pub axis: DividerAxis,
}

/// Thin themed separator line. Renders as a 1px rule along the chosen
/// `axis`, picking up the theme's border color.
#[component]
pub fn Divider(props: &DividerProps) -> Element {
    // Route `axis` into the stylesheet's own variant sink: a static value
    // takes the build-time fast path, a live one is passed via `derived(...)`
    // so the generated builder emits `StyleSource::Reactive` and re-resolves
    // the variant on change (its `is_reactive()` gates that). Reading
    // `axis.get()` INSIDE the `derived` closure is what subscribes the
    // apply-style Effect to the source.
    let style = match props.axis.clone() {
        Reactive::Static(axis) => DividerStyle().axis(axis),
        dynamic => DividerStyle().axis(derived(move || dynamic.get())),
    };
    ui! { view(style = style) {} }
}
