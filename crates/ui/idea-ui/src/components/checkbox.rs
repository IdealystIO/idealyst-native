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
    component, ui, Element, IntoElement, Reactive, Signal, StyleApplication,
};

use idea_theme::extensible::{installed_checkbox_sheets, ToneRef, VariantRef};

use crate::components::ControlSize;
use crate::stylesheets::{ControlRow, FieldLabel};

/// Unicode check mark glyph rendered in the box when checked.
const CHECK_GLYPH: &str = "\u{2713}";

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
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
        }
    }
}

#[component]
pub fn Checkbox(props: &CheckboxProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let size_key = props.size.as_variant_str().to_string();
    let appearance = format!("{}_{}", props.tone.key(), props.variant.key());

    let sheets = installed_checkbox_sheets();

    // Checkmark glyph — mounted only while checked, tinted to the
    // variant foreground by the glyph sheet's appearance arm.
    let glyph_sheet = sheets.glyph_sheet.clone();
    let glyph_appearance = appearance.clone();
    let glyph_size = size_key.clone();
    let glyph = runtime_core::switch(
        move || value.get(),
        move |on: &bool| {
            if *on {
                let gs = glyph_sheet.clone();
                let ga = glyph_appearance.clone();
                let gz = glyph_size.clone();
                runtime_core::text(CHECK_GLYPH)
                    .with_style(move || {
                        StyleApplication::new(gs.clone())
                            .with("appearance", ga.clone())
                            .with("size", gz.clone())
                    })
                    .into_element()
            } else {
                ui! { view {} }.into_element()
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
    runtime_core::pressable(kids, toggle)
        .with_style(|| StyleApplication::new(ControlRow::sheet()))
        .into_element()
}
