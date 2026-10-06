//! `Radio` — a single radio button (ring + dot + optional label).
//! `RadioGroup` — a controlled set of radios over a `Signal<String>`,
//! with single-select coordination.
//!
//! ```ignore
//! // Controlled group — the common case.
//! let plan = signal("pro".to_string());
//! ui! {
//!     RadioGroup(
//!         value = plan,
//!         on_change = move |id: String| plan.set(id),
//!         options = vec![
//!             RadioOption::new("free", "Free"),
//!             RadioOption::new("pro",  "Pro"),
//!             RadioOption::new("team", "Team"),
//!         ],
//!         tone = tone::Primary,
//!     )
//! }
//!
//! // Standalone radio for custom layouts.
//! ui! { Radio(label = Some("Email".into()), selected = picked, on_select = on_pick) }
//! ```
//!
//! Like Checkbox, drawn from primitives so it shares `tone` × `variant`
//! × `size`, and likewise the ring — not the label row — is the
//! `pressable` that takes focus and wears the focus ring. The selected
//! indicator is a filled dot inside a
//! tone-colored ring; override the appearance via
//! `install_radio_sheets(RadioSheetBuilder::new().add_tone(Hype).build())`.
//!
//! `disabled` follows `Switch` (and `Button`) on both `Radio` and
//! `RadioGroup` (where it disables every option): it blocks selection
//! through the ring pressable's own disabled binding (so `on_select` /
//! `on_change` never fire, and the host gets the native/a11y disabled
//! state), guards the label row's tap the same way, and dims the ring via
//! the sheet's `dimmed` axis.

use std::rc::Rc;

use runtime_core::{
    accessibility::Role, component, tap, ui, Element, IdealystSchema, IntoElement, Reactive,
    Signal, StyleApplication, TapRecognizer,
};

use idea_theme::extensible::{installed_radio_sheets, RadioSheets, ToneRef, VariantRef};

use crate::components::ControlSize;
use crate::components::stack::{Stack, StackAxis, StackGap};
use crate::stylesheets::{ControlRow, FieldLabel};

// =============================================================================
// Shared indicator + row builders
// =============================================================================

/// Build the ring+dot indicator. `is_selected` is read reactively, so
/// the ring re-tints and the dot mounts/unmounts as selection changes.
/// `appearance`/`size_key` are CLOSURES read live inside each style sink so
/// a reactive tone/variant/size re-styles the indicator in place.
///
/// The ring is the `pressable` (the row around it is a plain view), so it
/// is the keyboard-focusable host and the outer sheet's `__state_focused`
/// ring draws around the indicator alone — not around indicator + label.
fn radio_indicator(
    is_selected: impl Fn() -> bool + Clone + 'static,
    on_select: Rc<dyn Fn()>,
    disabled: Reactive<bool>,
    a11y_label: Option<String>,
    appearance: impl Fn() -> String + Clone + 'static,
    size_key: impl Fn() -> String + Clone + 'static,
    sheets: RadioSheets,
) -> Element {
    // Inner dot — mounted only while selected. Its style reads the live
    // appearance/size so a reactive tone/variant/size re-styles it in place.
    let dot_sheet = sheets.dot_sheet.clone();
    let dot_appearance = appearance.clone();
    let dot_size = size_key.clone();
    let dot_style = move || {
        StyleApplication::new(dot_sheet.clone())
            .with("appearance", dot_appearance())
            .with("size", dot_size())
    };
    let sel_for_dot = is_selected.clone();
    let dot = ui! {
        if sel_for_dot() {
            view(style = dot_style.clone()) {}
        }
    };

    // Outer ring — the pressable host. `disabled` is read LIVE in the style
    // so the dim follows a reactive prop in place (the `dimmed` axis —
    // Switch/Button parity).
    let outer_sheet = sheets.outer_sheet.clone();
    let sel_for_ring = is_selected;
    let disabled_dim = disabled.clone();
    let ring = runtime_core::pressable(vec![dot], move || (on_select)())
        .with_style(move || {
            StyleApplication::new(outer_sheet.clone())
                .with("appearance", appearance())
                .with("checked", if sel_for_ring() { "on" } else { "off" }.to_string())
                .with("size", size_key())
                .with("dimmed", if disabled_dim.get() { "on" } else { "off" }.to_string())
        })
        .a11y_role(Role::RadioButton);
    // Block the press through the pressable's own `disabled` binding (see
    // Switch): press block, host `set_disabled`, DISABLED state bit.
    // Attached only when the radio can be disabled — a `Static(false)`
    // radio carries no binding, like Button.
    let ring = match disabled {
        Reactive::Static(false) => ring,
        Reactive::Static(true) => ring.disabled(true),
        live => ring.disabled(move || live.get()),
    };
    // The label sits outside the pressable, so the ring can't derive its
    // accessible name from child content — name it explicitly. Snapshot: a
    // `Reactive` label's later values don't re-announce (the a11y prop bag
    // is plain data, not reactive).
    let ring = match a11y_label {
        Some(text) => ring.a11y_label(text),
        None => ring,
    };
    ring.into_element()
}

/// A clickable indicator + optional label row. The indicator owns the
/// press (and the focus ring); the row carries a tap handler so clicking
/// the label selects too — the indicator's own recognizer consumes its
/// taps first, so a tap on the ring fires exactly once.
fn radio_row(
    is_selected: impl Fn() -> bool + Clone + 'static,
    label: Option<(Element, Option<String>)>,
    on_select: Rc<dyn Fn()>,
    disabled: Reactive<bool>,
    appearance: impl Fn() -> String + Clone + 'static,
    size_key: impl Fn() -> String + Clone + 'static,
    sheets: RadioSheets,
) -> Element {
    let (label_el, label_text) = match label {
        Some((el, text)) => (Some(el), text),
        None => (None, None),
    };
    let row_disabled = disabled.clone();
    let indicator = radio_indicator(
        is_selected,
        on_select.clone(),
        disabled,
        label_text,
        appearance,
        size_key,
        sheets,
    );
    // No label — the indicator IS the whole control; skip the wrapper row.
    let Some(label_el) = label_el else { return indicator };

    // Builder form, not `ui!`: the `ui!` `view` emitter takes only
    // `style`/`test_id`/a11y props and DROPS anything else, so an
    // `on_touch = …` attribute there would silently never attach.
    //
    // The row is a plain view, not a pressable, so it has no disabled
    // binding of its own — the tap checks the live `disabled` itself.
    let row_tap = tap(TapRecognizer::new(), move || {
        if !row_disabled.get() {
            (on_select)()
        }
    });
    runtime_core::view(vec![indicator, label_el])
        .with_style(|| StyleApplication::new(ControlRow::sheet()))
        .on_touch(move |ev| row_tap(ev))
        .into_element()
}

// =============================================================================
// Radio (standalone)
// =============================================================================

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>` (tone/variant/size/disabled), so a `ui!` call site can pass
// a `Signal`/`rx!` and re-style in place. The controlled `selected` `Signal`
// stays bare (a reactive *source*), `on_select` is a handler, `label` is
// already `Reactive`.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct RadioProps {
    /// Optional label rendered to the right of the radio.
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub label: Reactive<Option<String>>,
    /// Whether this radio is currently selected.
    pub selected: Signal<bool>,
    /// Fires when the user clicks the radio. A standalone Radio does
    /// not own exclusivity — the host (or a RadioGroup) coordinates it.
    pub on_select: Rc<dyn Fn()>,
    /// Semantic palette for the selected ring + dot. Default Primary.
    pub tone: ToneRef,
    /// Surface skeleton. Default Filled.
    pub variant: VariantRef,
    /// Indicator scale. Default Md.
    pub size: ControlSize,
    /// When `true`, blocks selection (`on_select` never fires, from the ring
    /// or the label row) and dims the ring — the same opacity drop a disabled
    /// `Button` gets — and marks the ring disabled for the host (native
    /// disabled state / a11y). Default `false`. Reactive: pass a
    /// `Signal<bool>`/`rx!` and the radio enables and disables in place.
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub disabled: bool,
}

impl Default for RadioProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(None),
            selected: runtime_core::signal(false),
            on_select: Rc::new(|| {}),
            tone: Reactive::Static(ToneRef::default()),
            variant: Reactive::Static(VariantRef::default()),
            size: Reactive::Static(ControlSize::default()),
            disabled: Reactive::Static(false),
        }
    }
}

/// Renders a single radio button: a tone-colored ring with a filled inner
/// dot that mounts only while selected, plus an optional label, in a
/// clickable row.
#[component]
pub fn Radio(props: &RadioProps) -> Element {
    let selected = props.selected;
    // Style keys as live closures so a reactive tone/variant/size re-styles
    // the indicator in place; bare props collapse to a static resolution.
    let appearance = {
        let tone = props.tone.clone();
        let variant = props.variant.clone();
        move || format!("{}_{}", tone.get().key(), variant.get().key())
    };
    let size = props.size.clone();
    let size_key = move || size.get().as_variant_str().to_string();
    let label = crate::components::optional_reactive_text(props.label.clone(), FieldLabel())
        .map(|el| (el, props.label.get()));
    radio_row(
        move || selected.get(),
        label,
        props.on_select.clone(),
        props.disabled.clone(),
        appearance,
        size_key,
        installed_radio_sheets(),
    )
}

// =============================================================================
// RadioGroup
// =============================================================================

/// One option in a [`RadioGroup`]. `RadioOption::new(id, label)`.
#[derive(Clone, IdealystSchema, runtime_core::Remote)]
pub struct RadioOption {
    /// Stable identity for this option; matched against the group's
    /// `value` to decide selection and handed to `on_change` on tap.
    pub id: String,
    /// Row label. `Reactive<String>` — static or live (signal/`rx!`).
    #[schema(constraint = "reactive: static String or Signal/rx!")]
    pub label: Reactive<String>,
}

impl RadioOption {
    pub fn new(id: impl Into<String>, label: impl Into<Reactive<String>>) -> Self {
        Self { id: id.into(), label: label.into() }
    }
}

/// Layout direction for a [`RadioGroup`].
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
#[derive(IdealystSchema, runtime_core::Remote)]
pub enum RadioAxis {
    /// Stack options vertically. The default.
    #[default]
    Column,
    /// Lay options out in a row.
    Row,
}

impl runtime_core::VariantEnum for RadioAxis {
    fn as_variant_str(self) -> &'static str {
        match self {
            RadioAxis::Column => "column",
            RadioAxis::Row => "row",
        }
    }
    fn all_variants() -> &'static [Self] {
        &[RadioAxis::Column, RadioAxis::Row]
    }
}

// Reactive-by-default: `#[props]` wraps the scalar-DATA style props
// (tone/variant/size/disabled). The controlled `value` `Signal` stays bare,
// `on_change` is a handler, and `options` is a `Vec` (auto-skipped — bare).
// `axis` drives STRUCTURE (it selects the Stack layout branch) and so isn't
// routed reactively here — see the body TODO.
#[runtime_core::props]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct RadioGroupProps {
    /// The selected option's id. The host owns the signal.
    pub value: Signal<String>,
    /// Fires with the picked id when the user selects an option.
    pub on_change: Rc<dyn Fn(String)>,
    /// Options in render order.
    #[cfg_attr(feature = "docs", doc_control(skip))]
    pub options: Vec<RadioOption>,
    /// Layout direction. Default Column.
    pub axis: RadioAxis,
    /// Semantic palette applied to every option. Default Primary.
    pub tone: ToneRef,
    /// Surface skeleton applied to every option. Default Filled.
    pub variant: VariantRef,
    /// Indicator scale applied to every option. Default Md.
    pub size: ControlSize,
    /// When `true`, disables every option: selection is blocked
    /// (`on_change` never fires), each ring dims — the same opacity drop a
    /// disabled `Button` gets — and is marked disabled for the host (native
    /// disabled state / a11y). Default `false`. Reactive: pass a
    /// `Signal<bool>`/`rx!` and the group enables and disables in place.
    #[schema(constraint = "reactive: static bool or Signal/rx!")]
    pub disabled: bool,
}

impl Default for RadioGroupProps {
    fn default() -> Self {
        Self {
            value: runtime_core::signal(String::new()),
            on_change: Rc::new(|_| {}),
            options: Vec::new(),
            axis: Reactive::Static(RadioAxis::default()),
            tone: Reactive::Static(ToneRef::default()),
            variant: Reactive::Static(VariantRef::default()),
            size: Reactive::Static(ControlSize::default()),
            disabled: Reactive::Static(false),
        }
    }
}

/// Renders a controlled set of radios over a `Signal<String>`, enforcing
/// single-select: each option is a [`Radio`]-style row that reports its id
/// via `on_change`, and the row whose id matches `value` shows selected.
/// Options are stacked in a column or row per `axis`.
#[component]
pub fn RadioGroup(props: RadioGroupProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    // Style keys as live closures, cloned per row so a reactive tone/variant/
    // size re-styles every option's indicator in place.
    let appearance = {
        let tone = props.tone.clone();
        let variant = props.variant.clone();
        move || format!("{}_{}", tone.get().key(), variant.get().key())
    };
    let size = props.size.clone();
    let size_key = move || size.get().as_variant_str().to_string();
    let sheets = installed_radio_sheets();
    let disabled = props.disabled.clone();

    // One option's row: reports its id via `on_change`, shows selected while
    // `value` matches it.
    let option_row = move |option: RadioOption| -> Element {
        let id = option.id.clone();
        let id_for_select = option.id;
        let on_change_for_row = on_change.clone();
        let on_select: Rc<dyn Fn()> = Rc::new(move || (on_change_for_row)(id_for_select.clone()));

        let a11y_label = option.label.get();
        let label = runtime_core::text(option.label)
            .with_style(|| StyleApplication::new(FieldLabel::sheet()))
            .into_element();

        radio_row(
            move || value.get() == id,
            Some((label, Some(a11y_label))),
            on_select,
            disabled.clone(),
            appearance.clone(),
            size_key.clone(),
            sheets.clone(),
        )
    };

    // TODO(reactive-sweep): `axis` drives STRUCTURE (which Stack layout branch
    // is built), so a reactive axis won't re-lay-out without a `when()`/`switch`
    // around the Stack. Read once for now; the common case is a fixed axis.
    let stack_axis = match props.axis.get() {
        RadioAxis::Column => StackAxis::Column,
        RadioAxis::Row => StackAxis::Row,
    };
    let options = props.options;
    ui! {
        Stack(gap = StackGap::Sm, axis = stack_axis) {
            for option in options {
                option_row(option)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{classify, P};
    use idea_theme::testing::with_test_world;
    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::StyleApplication as App;

    fn has_focus_arm(app: &App) -> bool {
        app.sheet
            .variant_keys()
            .iter()
            .any(|(axis, _)| axis == "__state_focused")
    }

    /// Mirror of the Checkbox regression: the focus ring rings the RING,
    /// not the ring+label row. Before this, the row was the `pressable` and
    /// `ControlRow` carried the `state focused` border, so tabbing to a
    /// radio drew a border around its label text too.
    #[test]
    fn regression_focus_ring_rings_the_indicator_not_the_label_row() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let props = RadioProps {
                label: Reactive::Static(Some("Email".into())),
                ..Default::default()
            };
            let (children, row_style, row_tap) = match classify(Radio(&props)) {
                P::View {
                    children,
                    style,
                    on_touch,
                    ..
                } => (children, style, on_touch),
                _ => panic!("a labelled Radio renders a plain View row"),
            };
            assert!(row_tap, "the row still selects when the label is clicked");
            assert!(
                !has_focus_arm(&row_style.expect("row is styled").application()),
                "the label row declares no focus overlay — it is not the focus target"
            );

            let (ring_style, a11y) = match classify(children.into_iter().next().unwrap()) {
                P::Pressable {
                    style,
                    accessibility,
                    ..
                } => (style, accessibility),
                _ => panic!("the ring is the Pressable (the focusable host)"),
            };
            assert!(
                has_focus_arm(&ring_style.expect("ring is styled").application()),
                "the ring's own sheet carries the focus ring"
            );
            assert_eq!(
                a11y.role,
                Some(runtime_core::accessibility::Role::RadioButton)
            );
            assert_eq!(a11y.label.as_deref(), Some("Email"));
        });
    }

    /// Pressing the ring reports the selection exactly once.
    #[test]
    fn pressing_the_ring_selects() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let hits = Rc::new(std::cell::Cell::new(0u32));
            let sink = hits.clone();
            let props = RadioProps {
                label: Reactive::Static(Some("Email".into())),
                on_select: Rc::new(move || sink.set(sink.get() + 1)),
                ..Default::default()
            };
            let children = match classify(Radio(&props)) {
                P::View { children, .. } => children,
                _ => panic!("labelled Radio renders a row"),
            };
            match classify(children.into_iter().next().unwrap()) {
                P::Pressable { on_click, .. } => on_click(),
                _ => panic!("the ring is the Pressable"),
            }
            assert_eq!(hits.get(), 1);
        });
    }

    /// With no label there is nothing to lay out beside the ring, so the
    /// wrapper row is skipped entirely.
    #[test]
    fn unlabelled_radio_is_the_bare_ring() {
        with_test_world(|| {
            install_idea_theme(light_theme());
            let props = RadioProps::default();
            assert!(
                matches!(classify(Radio(&props)), P::Pressable { .. }),
                "an unlabelled Radio is the ring pressable itself"
            );
        });
    }

    /// Drive one tap (touch down + up in place) through a view's real
    /// `on_touch` handler.
    fn tap_row(row: Element) {
        use runtime_core::{TouchEvent, TouchId, TouchPhase, TouchPoint};
        let mut el = row;
        while let Element::Owned { element, .. } = el {
            el = *element;
        }
        let handler = match el {
            Element::Item { data, .. } => data
                .downcast_ref::<runtime_vocabulary::prims::PrimCell<runtime_vocabulary::prims::ViewPrim>>()
                .expect("a labelled Radio renders a View row")
                .take()
                .on_touch
                .expect("the row carries on_touch"),
            _ => panic!("a labelled Radio renders a View item"),
        };
        for phase in [TouchPhase::Began, TouchPhase::Ended] {
            handler(&TouchEvent {
                id: TouchId(1),
                phase,
                position: TouchPoint::new(1.0, 1.0),
                window_position: TouchPoint::new(1.0, 1.0),
                timestamp_ns: 0,
                force: None,
            });
        }
    }

    fn row_tap_hits(disabled: Reactive<bool>) -> u32 {
        let hits = Rc::new(std::cell::Cell::new(0u32));
        with_test_world(|| {
            install_idea_theme(light_theme());
            let sink = hits.clone();
            let props = RadioProps {
                label: Reactive::Static(Some("Email".into())),
                on_select: Rc::new(move || sink.set(sink.get() + 1)),
                disabled,
                ..Default::default()
            };
            tap_row(Radio(&props));
        });
        hits.get()
    }

    /// Regression guard for `disabled`: the label row is a plain view with
    /// its own tap recognizer, OUTSIDE the ring pressable's disabled
    /// binding — so without its own check, tapping the label of a disabled
    /// Radio still fired `on_select`.
    #[test]
    fn regression_disabled_radio_label_row_tap_does_not_fire() {
        assert_eq!(row_tap_hits(Reactive::Static(false)), 1, "an enabled row tap fires");
        assert_eq!(row_tap_hits(Reactive::Static(true)), 0, "a disabled row tap is ignored");
    }
}
