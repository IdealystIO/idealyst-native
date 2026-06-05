//! `SegmentedControl` — a row of mutually-exclusive options.
//!
//! The "iOS segmented picker" pattern: a short, fixed row where exactly
//! one segment is selected at a time. Before this existed you hand-rolled
//! it as a `Stack(axis = Row)` of `Button`s and tracked selection
//! yourself; `SegmentedControl` packages that into one controlled
//! component.
//!
//! ```ignore
//! let view = signal!("list".to_string());
//! let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| view.set(v));
//! ui! {
//!     SegmentedControl(
//!         value = view,
//!         on_change = on_change,
//!         options = vec![
//!             SegmentOption::new("list", "List"),
//!             SegmentOption::new("grid", "Grid"),
//!             SegmentOption::new("map", "Map"),
//!         ],
//!     )
//! }
//! ```
//!
//! Like [`Select`](super::select::Select) and the other selection
//! controls, it's **controlled by value**: the host owns a
//! `Signal<String>` holding the selected option's `id`, and `on_change`
//! commits the newly-picked `id`. The segment whose `id` equals the
//! current `value` paints selected; mutual exclusivity is automatic
//! because exactly one `id` can equal `value`.
//!
//! ## Appearance
//! Reuses the [`Tabs`](super::tabs::Tabs) stylesheets — the segmented row
//! is a `TabBar` container and each segment a `TabButton` whose `active`
//! axis flips between `on` (selected) and `off`. That keeps the selected
//! highlight reactive and theme-driven without a bespoke sheet.

use std::rc::Rc;

use runtime_core::{
    component, pressable, recipe, text, ui, Element, IdealystSchema, Reactive, Signal,
    StyleApplication,
};

use crate::stylesheets::{TabBar, TabButton};

/// One segment in a [`SegmentedControl`]. `id` is the value committed to
/// the bound signal when this segment is chosen; `label` is what the user
/// sees.
#[derive(Clone, IdealystSchema)]
pub struct SegmentOption {
    /// Stable value committed to the control's `value` signal when this
    /// segment is chosen. Compared against the current value to mark the
    /// selected segment.
    pub id: String,
    /// Segment label. `Reactive<String>` — static or live (signal/`rx!`).
    pub label: Reactive<String>,
}

impl SegmentOption {
    pub fn new(id: impl Into<String>, label: impl Into<Reactive<String>>) -> Self {
        Self { id: id.into(), label: label.into() }
    }
}

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct SegmentedControlProps {
    /// Controlled selected value — the `id` of the chosen
    /// [`SegmentOption`]. The host owns the signal; tapping a segment sets
    /// it via `on_change`. The segment whose `id` equals this paints
    /// selected.
    pub value: Signal<String>,
    /// Fires with the chosen segment's `id` when the user taps a segment.
    /// Default is a no-op so an unwired control doesn't silently mutate —
    /// pass `move |id| value.set(id)` to make taps switch segments.
    pub on_change: Rc<dyn Fn(String)>,
    /// The segments, left-to-right.
    pub options: Vec<SegmentOption>,
}

impl Default for SegmentedControlProps {
    // Manual impl: `Signal<String>` and `Rc<dyn Fn(String)>` don't derive
    // `Default`. Mirrors `SelectProps` / `TabsProps`.
    fn default() -> Self {
        Self {
            value: Signal::new(String::new()),
            on_change: Rc::new(|_| {}),
            options: Vec::new(),
        }
    }
}

/// Renders a row of mutually-exclusive segments with reactive selected
/// highlighting. Pure UI: it draws one pressable per `SegmentOption`,
/// lights the one whose `id` matches `value`, and reports taps via
/// `on_change` — the host owns the selected value.
#[component]
pub fn SegmentedControl(props: SegmentedControlProps) -> Element {
    let options = props.options;
    let value = props.value;
    let on_change = props.on_change;

    let container_style = TabBar();

    // `Pressable` isn't a ui!-level tag (the framework macro omits it so
    // idea-ui owns the styled wrapper), so each segment is built via the
    // builder fns and collected — the same shape `Tabs` uses for its
    // per-tab buttons.
    let mut segments: Vec<Element> = Vec::with_capacity(options.len());
    for option in options {
        let id = option.id;
        let label = option.label;

        // Each segment captures its own `id` + `on_change` so the press
        // commits the right value regardless of how many segments exist.
        let on_change_for_seg = on_change.clone();
        let id_for_press = id.clone();
        let press = move || on_change_for_seg(id_for_press.clone());

        // Reactive style: re-runs whenever `value` fires, flipping the
        // `active` axis between `on` (this segment is selected) and `off`.
        let id_for_style = id.clone();
        let seg_style = move || {
            let active = if value.get() == id_for_style { "on" } else { "off" };
            StyleApplication::new(TabButton::sheet()).with("active", active.to_string())
        };

        let label_el: Element = text(label).into();
        let seg: Element = pressable(vec![label_el], press).with_style(seg_style).into();
        segments.push(seg);
    }

    ui! { view(style = container_style) { segments } }
}

recipe!(
    SegmentedControl,
    /// A controlled segmented picker. The host owns the `value` signal
    /// (the selected segment's `id`); `on_change` writes the picked id
    /// back. Build the segments with `SegmentOption::new(id, label)`.
    pub fn segmented_control_view_switch() -> ::runtime_core::Element {
        use crate::components::segmented_control::{SegmentOption, SegmentedControl};
        use ::runtime_core::{signal, ui};
        use ::std::rc::Rc;

        let view = signal!("list".to_string());
        let on_change: Rc<dyn Fn(String)> = Rc::new(move |v| view.set(v));
        ui! {
            SegmentedControl(
                value = view,
                on_change = on_change,
                options = vec![
                    SegmentOption::new("list", "List"),
                    SegmentOption::new("grid", "Grid"),
                    SegmentOption::new("map", "Map"),
                ],
            )
        }
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_empty_and_inert() {
        let p = SegmentedControlProps::default();
        assert!(p.options.is_empty());
        assert_eq!(p.value.get(), String::new());
    }

    /// One pressable segment per option, wrapped in a single row view.
    #[test]
    fn builds_one_segment_per_option() {
        let el = SegmentedControl(SegmentedControlProps {
            options: vec![
                SegmentOption::new("a", "A"),
                SegmentOption::new("b", "B"),
                SegmentOption::new("c", "C"),
            ],
            ..Default::default()
        });
        let children = match &el {
            Element::View { children, .. } => children,
            _ => panic!("SegmentedControl should render a row View"),
        };
        assert_eq!(children.len(), 3, "one segment per option");
        assert!(
            children.iter().all(|c| matches!(c, Element::Pressable { .. })),
            "each segment must be a pressable"
        );
    }
}
