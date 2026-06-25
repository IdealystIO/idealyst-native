//! `Checkbox` — a square box with a checkmark, plus an optional label.
//!
//! ```ignore
//! ui! {
//!     Checkbox(
//!         label = Some("I agree to the terms".into()),
//!         value = agreed,
//!         on_change = move |v: bool| agreed.set(v),
//!         tone = tone::Primary,
//!     )
//! }
//! ```
//!
//! Drawn from primitives (`pressable` row + `view` box + checkmark
//! `text`) so it shares the `tone` × `variant` × `size` axes with the
//! rest of idea-ui. The box's selected fill is the tone/variant render
//! (`variant::Filled` → solid, `Soft` → tint, `Outlined` → bordered);
//! unselected it's a muted outline. Override the appearance via
//! `install_checkbox_sheets(CheckboxSheetBuilder::new().add_tone(Hype).build())`.

use std::rc::Rc;

use runtime_core::{
    component, icon, resolve_style, ui, Element, IconData, IdealystSchema, IntoElement, Reactive,
    Signal, StyleApplication,
};

use idea_theme::extensible::{installed_checkbox_sheets, ToneRef, VariantRef};

use crate::components::ControlSize;
use crate::stylesheets::{ControlRow, FieldLabel};

/// Unicode check mark glyph rendered in the box when checked.
const CHECK_GLYPH: &str = "\u{2713}";

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct CheckboxProps {
    /// Optional label rendered to the right of the box.
    /// `Reactive<Option<String>>` — static or live.
    pub label: Reactive<Option<String>>,
    /// Controlled checked state. The host owns the signal.
    pub value: Signal<bool>,
    /// Fires with the new value when the user toggles the box.
    pub on_change: Rc<dyn Fn(bool)>,
    /// Semantic palette for the checked fill. Default Primary.
    pub tone: ToneRef,
    /// Surface skeleton for the checked fill. Default Filled.
    pub variant: VariantRef,
    /// Box scale. Default Md.
    pub size: ControlSize,
    /// Optional custom checked-state icon, shown in place of the default
    /// checkmark glyph (e.g. `icons_lucide::CHECK` or a task-specific mark).
    /// Inherits the checkmark's foreground color. `None` = the default ✓.
    pub icon: Option<IconData>,
    /// Optional robot/E2E test id, forwarded to the interactive row. Only
    /// honored when idea-ui's `robot` feature is on; ignored otherwise.
    pub test_id: Option<&'static str>,
}

impl Default for CheckboxProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(None),
            value: Signal::new(false),
            on_change: Rc::new(|_| {}),
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            size: ControlSize::default(),
            icon: None,
            test_id: None,
        }
    }
}

/// Renders a tappable row: a tone/variant-styled box that shows a
/// checkmark when `value` is true, plus the optional `label`. Tapping
/// anywhere on the row fires `on_change` with the toggled value.
#[component]
pub fn Checkbox(props: &CheckboxProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let size_key = props.size.as_variant_str().to_string();
    let appearance = format!("{}_{}", props.tone.key(), props.variant.key());

    let sheets = installed_checkbox_sheets();

    // Checkmark — mounted only while checked, tinted to the variant
    // foreground by the glyph sheet's appearance arm. A custom `icon`
    // replaces the default ✓ glyph, inheriting the same foreground.
    let glyph_sheet = sheets.glyph_sheet.clone();
    let glyph_appearance = appearance.clone();
    let glyph_size = size_key.clone();
    let icon_data = props.icon;
    let glyph = runtime_core::switch(
        move || value.get(),
        move |on: &bool| {
            if !*on {
                return ui! { view {} }.into_element();
            }
            let gs = glyph_sheet.clone();
            let ga = glyph_appearance.clone();
            let gz = glyph_size.clone();
            match icon_data {
                Some(data) => {
                    // Resolve the checkmark foreground and stamp it on the icon
                    // (native icons don't inherit text color — see Button).
                    let fg = resolve_style(
                        &StyleApplication::new(gs).with("appearance", ga).with("size", gz),
                    )
                    .color
                    .clone();
                    let el = icon(data).size(14.0);
                    match fg {
                        Some(c) => el.color(move || c.resolve()).into_element(),
                        None => el.into_element(),
                    }
                }
                None => runtime_core::text(CHECK_GLYPH)
                    .with_style(move || {
                        StyleApplication::new(gs.clone())
                            .with("appearance", ga.clone())
                            .with("size", gz.clone())
                    })
                    .into_element(),
            }
        },
    );

    // The box — fill flips between the tone appearance (checked) and
    // the muted outline (unchecked) via the `checked` axis.
    let box_sheet = sheets.box_sheet.clone();
    let box_appearance = appearance;
    let box_size = size_key;
    let box_el = runtime_core::view(vec![glyph])
        .with_style(move || {
            StyleApplication::new(box_sheet.clone())
                .with("appearance", box_appearance.clone())
                .with("checked", if value.get() { "on" } else { "off" }.to_string())
                .with("size", box_size.clone())
        })
        .into_element();

    let mut kids: Vec<Element> = Vec::with_capacity(2);
    kids.push(box_el);
    if let Some(label) = crate::components::optional_reactive_text(props.label.clone(), FieldLabel())
    {
        kids.push(label);
    }

    let toggle = move || (on_change)(!value.get());
    let row = runtime_core::pressable(kids, toggle)
        .with_style(|| StyleApplication::new(ControlRow::sheet()));
    // Forward the test id to the interactive row for robot/E2E location.
    // Gated: `.test_id()` only exists under `runtime-core/robot`.
    #[cfg(feature = "robot")]
    let row = match props.test_id {
        Some(tid) => row.test_id(tid),
        None => row,
    };
    row.into_element()
}
