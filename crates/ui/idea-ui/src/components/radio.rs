//! `Radio` — a single radio button (ring + dot + optional label).
//! `RadioGroup` — a controlled set of radios over a `Signal<String>`,
//! with single-select coordination.
//!
//! ```ignore
//! // Controlled group — the common case.
//! let plan = signal!("pro".to_string());
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
//! × `size`. The selected indicator is a filled dot inside a
//! tone-colored ring; override the appearance via
//! `install_radio_sheets(RadioSheetBuilder::new().add_tone(Hype).build())`.

use std::rc::Rc;

use runtime_core::{
    component, ui, Element, IdealystSchema, IntoElement, Reactive, Signal, StyleApplication,
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
fn radio_indicator(
    is_selected: impl Fn() -> bool + Clone + 'static,
    appearance: String,
    size_key: String,
    sheets: RadioSheets,
) -> Element {
    // Inner dot — mounted only while selected.
    let dot_sheet = sheets.dot_sheet.clone();
    let dot_appearance = appearance.clone();
    let dot_size = size_key.clone();
    let sel_for_dot = is_selected.clone();
    let dot = runtime_core::switch(
        move || sel_for_dot(),
        move |on: &bool| {
            if *on {
                let ds = dot_sheet.clone();
                let da = dot_appearance.clone();
                let dz = dot_size.clone();
                runtime_core::view(Vec::new())
                    .with_style(move || {
                        StyleApplication::new(ds.clone())
                            .with("appearance", da.clone())
                            .with("size", dz.clone())
                    })
                    .into_element()
            } else {
                ui! { view {} }.into_element()
            }
        },
    );

    // Outer ring.
    let outer_sheet = sheets.outer_sheet.clone();
    let sel_for_ring = is_selected;
    runtime_core::view(vec![dot])
        .with_style(move || {
            StyleApplication::new(outer_sheet.clone())
                .with("appearance", appearance.clone())
                .with("checked", if sel_for_ring() { "on" } else { "off" }.to_string())
                .with("size", size_key.clone())
        })
        .into_element()
}

/// A clickable indicator + optional label row.
fn radio_row(
    is_selected: impl Fn() -> bool + Clone + 'static,
    label: Option<Element>,
    on_select: Rc<dyn Fn()>,
    appearance: String,
    size_key: String,
    sheets: RadioSheets,
) -> Element {
    let indicator = radio_indicator(is_selected, appearance, size_key, sheets);
    let mut kids: Vec<Element> = Vec::with_capacity(2);
    kids.push(indicator);
    if let Some(l) = label {
        kids.push(l);
    }
    runtime_core::pressable(kids, move || (on_select)())
        .with_style(|| StyleApplication::new(ControlRow::sheet()))
        .into_element()
}

// =============================================================================
// Radio (standalone)
// =============================================================================

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
}

impl Default for RadioProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(None),
            selected: Signal::new(false),
            on_select: Rc::new(|| {}),
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            size: ControlSize::default(),
        }
    }
}

/// Renders a single radio button: a tone-colored ring with a filled inner
/// dot that mounts only while selected, plus an optional label, in a
/// clickable row.
#[component]
pub fn Radio(props: &RadioProps) -> Element {
    let selected = props.selected;
    let appearance = format!("{}_{}", props.tone.key(), props.variant.key());
    let size_key = props.size.as_variant_str().to_string();
    let label = crate::components::optional_reactive_text(props.label.clone(), FieldLabel());
    radio_row(
        move || selected.get(),
        label,
        props.on_select.clone(),
        appearance,
        size_key,
        installed_radio_sheets(),
    )
}

// =============================================================================
// RadioGroup
// =============================================================================

/// One option in a [`RadioGroup`]. `RadioOption::new(id, label)`.
#[derive(Clone)]
#[derive(IdealystSchema)]
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
#[derive(IdealystSchema)]
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
}

impl Default for RadioGroupProps {
    fn default() -> Self {
        Self {
            value: Signal::new(String::new()),
            on_change: Rc::new(|_| {}),
            options: Vec::new(),
            axis: RadioAxis::default(),
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            size: ControlSize::default(),
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
    let appearance = format!("{}_{}", props.tone.key(), props.variant.key());
    let size_key = props.size.as_variant_str().to_string();
    let sheets = installed_radio_sheets();

    let mut rows: Vec<Element> = Vec::with_capacity(props.options.len());
    for option in props.options {
        let id = option.id.clone();
        let id_for_select = option.id.clone();
        let on_change_for_row = on_change.clone();
        let on_select: Rc<dyn Fn()> = Rc::new(move || (on_change_for_row)(id_for_select.clone()));

        let label = runtime_core::text(option.label)
            .with_style(|| StyleApplication::new(FieldLabel::sheet()))
            .into_element();

        rows.push(radio_row(
            move || value.get() == id,
            Some(label),
            on_select,
            appearance.clone(),
            size_key.clone(),
            sheets.clone(),
        ));
    }

    let gap = StackGap::Sm;
    match props.axis {
        RadioAxis::Column => ui! { Stack(gap = gap, axis = StackAxis::Column) { rows } },
        RadioAxis::Row => ui! { Stack(gap = gap, axis = StackAxis::Row) { rows } },
    }
}
