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
    component, pressable, recipe, resolve_style, text, ui, Element, IdealystSchema, Reactive,
    Signal, StyleApplication, StyleRules, StyleSheet,
};

use crate::stylesheets::{TabBar, TabButton};

thread_local! {
    static SEG_LABEL_BASE_SHEET: std::cell::RefCell<Option<Rc<StyleSheet>>> =
        const { std::cell::RefCell::new(None) };
}

/// A single shared, empty base sheet for segment labels. The per-state color
/// rides a `with_computed` layer keyed on the selected state, so the resolution
/// cache key (sheet Rc pointer + computed key) stays stable across renders —
/// mirrors `Tabs::tab_label_base_sheet`.
fn seg_label_base_sheet() -> Rc<StyleSheet> {
    SEG_LABEL_BASE_SHEET.with(|s| {
        if s.borrow().is_none() {
            *s.borrow_mut() = Some(Rc::new(StyleSheet::r#static(StyleRules::default())));
        }
        s.borrow().as_ref().cloned().unwrap()
    })
}

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

// Reactive-by-default: `#[props]` auto-skips EVERY field here — `value` is
// already `Reactive<String>`, `on_change` is a handler (`Rc`), and `options`
// is a `Vec`. No scalar-DATA prop to wrap, so the struct/Default/body are
// unchanged; the attribute is added for uniformity with the other controls.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct SegmentedControlProps {
    /// Controlled selected value — the `id` of the chosen [`SegmentOption`].
    /// `Reactive<String>` — a `Signal<String>` the host owns, or a model-derived
    /// `rx!(...)` that maps a typed enum/bool to the matching option `id` (so the
    /// control fits state that isn't a standalone string signal). Tapping a
    /// segment reports the new `id` via `on_change`; the segment whose `id`
    /// equals this paints selected.
    pub value: Reactive<String>,
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
            value: Reactive::Static(String::new()),
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
    let value = props.value; // Reactive<String> — cloned into each segment's style closure
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
        let value_style = value.clone();
        let seg_style = move || {
            let active = if value_style.get() == id_for_style { "on" } else { "off" };
            StyleApplication::new(TabButton::sheet()).with("active", active.to_string())
        };

        // The TabButton sheet's on/off foreground (the selected segment's accent
        // vs muted label, and the live theme color) lives on the pressable, but
        // native TextView/UILabel/NSTextField don't inherit text color from their
        // parent — only web's CSS cascade does. So resolve that color and stamp it
        // on the label NODE itself, reactively (re-runs on `value` + theme). Without
        // this the segment label renders in the widget-default color on native: it
        // never flips on selection AND never follows a light/dark swap. Mirrors
        // `Tabs`.
        let id_for_label = id.clone();
        let value_label = value.clone();
        let label_style = move || {
            let on = value_label.get() == id_for_label;
            let variant = if on { "on" } else { "off" };
            let app = StyleApplication::new(TabButton::sheet()).with("active", variant.to_string());
            let color = resolve_style(&app).color.clone();
            let key = if on { "seg_label_on" } else { "seg_label_off" };
            StyleApplication::new(seg_label_base_sheet()).with_computed(key, move || StyleRules {
                color: color.clone(),
                ..Default::default()
            })
        };

        let label_el: Element = text(label).with_style(label_style).into();
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
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, StyleSource};

    /// Resolved color on a segment's label text node (NOT the pressable's).
    fn seg_label_color(seg: &Element) -> Option<runtime_core::Color> {
        let label = match seg {
            Element::Pressable { children, .. } => &children[0],
            _ => panic!("a segment is a Pressable"),
        };
        match label {
            Element::Text { style, .. } => match style.as_ref()? {
                StyleSource::Reactive(f) => resolve_style(&f()).color.clone().map(|c| c.resolve()),
                StyleSource::Static(a) => resolve_style(a).color.clone().map(|c| c.resolve()),
                _ => None,
            },
            _ => panic!("a segment label is a Text node"),
        }
    }

    /// The color the TabButton sheet resolves for a given active state.
    fn tabbutton_color(active: &str) -> runtime_core::Color {
        let app = StyleApplication::new(TabButton::sheet()).with("active", active.to_string());
        resolve_style(&app)
            .color
            .clone()
            .expect("TabButton resolves a foreground")
            .resolve()
    }

    // Regression: the segment label was a bare text node whose color lived only on
    // the wrapping pressable, so on native (no CSS cascade) it rendered in the
    // widget default — the selected segment never took the accent color and the
    // labels never followed a theme swap. Each label must carry its OWN color
    // matching its selected state, like `Tabs`.
    #[test]
    fn regression_segment_labels_carry_their_own_active_color() {
        install_idea_theme(light_theme());
        let el = SegmentedControl(SegmentedControlProps {
            options: vec![SegmentOption::new("a", "A"), SegmentOption::new("b", "B")],
            value: Signal::new("a".to_string()).into(),
            ..Default::default()
        });
        let children = match &el {
            Element::View { children, .. } => children,
            _ => panic!("SegmentedControl renders a row View"),
        };
        let on = seg_label_color(&children[0]).expect("selected label carries a color");
        let off = seg_label_color(&children[1]).expect("unselected label carries a color");
        assert_eq!(on, tabbutton_color("on"), "selected segment label = TabButton `on`");
        assert_eq!(off, tabbutton_color("off"), "unselected segment label = TabButton `off`");
        assert_ne!(on, off, "selection must change the label color");
    }

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
