//! `Textarea` — multi-line text input. The multi-line sibling of
//! [`Field`](crate::components::field::Field): same label / helper /
//! error / tone / size surface, wrapping the framework's `text_area`
//! primitive instead of `text_input`.
//!
//! ```ignore
//! ui! {
//!     Textarea(
//!         label = Some("Bio".into()),
//!         value = bio,
//!         on_change = move |v: String| bio.set(v),
//!         placeholder = "Tell us about yourself…",
//!         rows = 5,
//!     )
//! }
//! ```
//!
//! `rows` sets the resting height (the box grows no shorter than
//! `rows` lines). It reuses Field's input + help stylesheets so a
//! Textarea sits flush next to a Field, with the line-count height
//! contributed as a `with_computed` layer (keyed by `rows`+`size` so
//! identical configs share one backend class).

use std::rc::Rc;

use runtime_core::{
    component, ui, IntoElement, Length, Element, Reactive, Signal, StyleApplication, StyleRules,
    Tokenized, VariantEnum,
};

use idea_theme::extensible::{tone as tones, ToneRef};

use crate::components::field::{field_help_sheet, field_input_sheet};
use crate::stylesheets::{FieldGroup, FieldLabel};
pub use crate::stylesheets::FieldSize;

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct TextareaProps {
    /// Optional label above the input.
    pub label: Reactive<Option<String>>,
    pub value: Signal<String>,
    pub on_change: Rc<dyn Fn(String)>,
    pub placeholder: Option<String>,
    /// Helper text below the input.
    pub help: Reactive<Option<String>>,
    /// Error text below the input; takes precedence over `help` and
    /// auto-applies the Danger tone.
    pub error: Reactive<Option<String>>,
    /// Optional tone overlay (border + help-text color).
    pub tone: Option<ToneRef>,
    pub size: FieldSize,
    /// Resting height in text lines. Default 3.
    pub rows: u32,
}

impl Default for TextareaProps {
    fn default() -> Self {
        Self {
            label: Reactive::Static(None),
            value: Signal::new(String::new()),
            on_change: Rc::new(|_| {}),
            placeholder: None,
            help: Reactive::Static(None),
            error: Reactive::Static(None),
            tone: None,
            size: FieldSize::default(),
            rows: 3,
        }
    }
}

/// `(line_height_px, vertical_chrome_px)` per size — used to translate
/// `rows` into a min-height. Mirrors Field's per-size padding/font.
fn size_metrics(size: FieldSize) -> (f32, f32) {
    match size.as_variant_str() {
        "sm" => (18.0, 8.0),
        "lg" => (28.0, 24.0),
        _ => (22.0, 16.0),
    }
}

#[component]
pub fn Textarea(props: &TextareaProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let placeholder = props.placeholder.clone();
    let size = props.size;
    let rows = props.rows.max(1);
    let has_error = props.error.get().is_some();

    let tone: Option<ToneRef> = props.tone.clone().or_else(|| {
        if has_error { Some(tones::Danger.into()) } else { None }
    });
    let tone_key = tone.as_ref().map(|t| t.key()).unwrap_or("default").to_string();
    let size_key = size.as_variant_str().to_string();

    let (line_px, chrome_px) = size_metrics(size);
    let min_height = rows as f32 * line_px + chrome_px;

    // STATIC style (reuse Field's sheets) + a computed min-height layer
    // keyed by rows+size so identical configs dedupe to one class.
    let input_style = StyleApplication::new(field_input_sheet())
        .with("size", size_key.clone())
        .with("tone", tone_key.clone())
        .with_computed(format!("ta-minh-{}-{}", rows, size_key), move || StyleRules {
            min_height: Some(Tokenized::Literal(Length::Px(min_height))),
            ..Default::default()
        });
    let help_style = StyleApplication::new(field_help_sheet()).with("tone", tone_key);

    let label_node = crate::components::optional_reactive_text(props.label.clone(), FieldLabel());

    let help_combined = match (props.error.clone(), props.help.clone()) {
        (Reactive::Static(e), Reactive::Static(h)) => Reactive::Static(e.or(h)),
        (e, h) => Reactive::Dynamic(Rc::new(move || e.get().or_else(|| h.get()))),
    };
    let help_node = crate::components::optional_reactive_text(help_combined, help_style);

    let mut input = runtime_core::text_area(value, move |v: String| (on_change)(v))
        .with_style(input_style);
    if let Some(p) = placeholder {
        input = input.placeholder(p);
    }
    let input_node = input.into_element();

    let mut children: Vec<Element> = Vec::with_capacity(3);
    if let Some(l) = label_node {
        children.push(l);
    }
    children.push(input_node);
    if let Some(h) = help_node {
        children.push(h);
    }

    ui! { view(style = FieldGroup()) { children } }
}
