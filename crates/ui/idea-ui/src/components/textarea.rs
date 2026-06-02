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
//!         rows = 3,
//!         max_rows = 8,
//!     )
//! }
//! ```
//!
//! The underlying `text_area` primitive is intrinsically sized to its
//! content, so a Textarea **grows to fit what's typed**. The two row
//! props bound that growth:
//!
//! - `rows` is the resting floor — the box is at least this tall and
//!   never shrinks below it (a `min_height`).
//! - `max_rows` is the ceiling — once content passes it the box stops
//!   growing and scrolls (a `max_height`). `0` (the default) leaves it
//!   uncapped, so it grows without bound.
//!
//! It reuses Field's input + help stylesheets so a Textarea sits flush
//! next to a Field, with the min/max-height contributed as a
//! `with_computed` layer (keyed by `rows`+`max_rows`+`size` so
//! identical configs share one backend class).

use std::rc::Rc;

use runtime_core::{
    component, ui, IdealystSchema, IntoElement, Length, Element, Reactive, Signal, StyleApplication,
    StyleRules, Tokenized, VariantEnum,
};

use idea_theme::extensible::{tone as tones, ToneRef};

use crate::components::field::{field_help_sheet, field_input_sheet};
use crate::stylesheets::{FieldGroup, FieldLabel};
pub use crate::stylesheets::{FieldAppearance, FieldSize};

#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
#[derive(IdealystSchema)]
pub struct TextareaProps {
    /// Optional label above the input.
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub label: Reactive<Option<String>>,
    /// Controlled text value. The host owns the signal.
    pub value: Signal<String>,
    /// Fires with the full new text on every edit.
    pub on_change: Rc<dyn Fn(String)>,
    /// Placeholder shown when the value is empty.
    pub placeholder: Option<String>,
    /// Helper text below the input.
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub help: Reactive<Option<String>>,
    /// Error text below the input; takes precedence over `help` and
    /// auto-applies the Danger tone.
    #[schema(constraint = "reactive: static Option<String> or Signal/rx!")]
    pub error: Reactive<Option<String>>,
    /// Optional tone overlay (border + help-text color).
    pub tone: Option<ToneRef>,
    /// Padding/font scale (Sm, Md, Lg). Default Md.
    pub size: FieldSize,
    /// Visual shell: `Outline` (bordered, default), `Contained` (filled),
    /// or `Bare` (no chrome). All three keep a focus ring.
    pub variant: FieldAppearance,
    /// Resting height in text lines — the floor the box grows from and
    /// never shrinks below. Default 3.
    #[schema(constraint = "text lines; floored at 1")]
    pub rows: u32,
    /// Maximum height in text lines before the box stops growing and
    /// scrolls. `0` (the default) leaves the autogrow uncapped.
    #[schema(constraint = "text lines; 0 = uncapped, otherwise clamped up to `rows`")]
    pub max_rows: u32,
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
            variant: FieldAppearance::default(),
            rows: 3,
            max_rows: 0,
        }
    }
}

/// `(line_height_px, vertical_chrome_px)` per size — used to translate
/// a row count into a pixel height. Mirrors Field's per-size padding/font.
fn size_metrics(size: FieldSize) -> (f32, f32) {
    match size.as_variant_str() {
        "sm" => (18.0, 8.0),
        "lg" => (28.0, 24.0),
        _ => (22.0, 16.0),
    }
}

/// Resolve the box's height bounds from the requested `rows` / `max_rows`
/// at a given size, returning `(min_height_px, max_height_px, rows,
/// max_rows)` where the trailing two are the *resolved* row counts (used
/// to key the generated style class).
///
/// - `rows` floors at 1 — a zero-row textarea is meaningless.
/// - `max_rows == 0` leaves the autogrow uncapped (`None`).
/// - a `max_rows` below the resting `rows` clamps up to `rows`, so "cap
///   below floor" degrades to "no growth past the floor" rather than an
///   inverted min > max.
fn height_bounds(rows: u32, max_rows: u32, size: FieldSize) -> (f32, Option<f32>, u32, u32) {
    let rows = rows.max(1);
    let max_rows = if max_rows == 0 { 0 } else { max_rows.max(rows) };
    let (line_px, chrome_px) = size_metrics(size);
    let min_height = rows as f32 * line_px + chrome_px;
    let max_height = if max_rows == 0 {
        None
    } else {
        Some(max_rows as f32 * line_px + chrome_px)
    };
    (min_height, max_height, rows, max_rows)
}

/// Renders a controlled multi-line text input with optional label,
/// helper/error text, and tone, auto-growing between the `rows` floor
/// and the `max_rows` cap.
#[component]
pub fn Textarea(props: &TextareaProps) -> Element {
    let value = props.value;
    let on_change = props.on_change.clone();
    let placeholder = props.placeholder.clone();
    let size = props.size;
    let has_error = props.error.get().is_some();

    let tone: Option<ToneRef> = props.tone.clone().or_else(|| {
        if has_error { Some(tones::Danger.into()) } else { None }
    });
    let tone_key = tone.as_ref().map(|t| t.key()).unwrap_or("default").to_string();
    let size_key = size.as_variant_str().to_string();

    let (min_height, max_height, rows, max_rows) =
        height_bounds(props.rows, props.max_rows, size);

    // STATIC style (reuse Field's sheets) + a computed min/max-height
    // layer keyed by rows+max_rows+size so identical configs dedupe to
    // one class. The primitive sizes the box to its content; these
    // bounds set the resting floor and the grow-then-scroll cap.
    let input_style = StyleApplication::new(field_input_sheet())
        .with("size", size_key.clone())
        .with("appearance", props.variant.as_variant_str().to_string())
        .with("tone", tone_key.clone())
        .with_computed(format!("ta-h-{}-{}-{}", rows, max_rows, size_key), move || StyleRules {
            min_height: Some(Tokenized::Literal(Length::Px(min_height))),
            max_height: max_height.map(|h| Tokenized::Literal(Length::Px(h))),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_bounds_uncapped_grows_from_floor() {
        let size = FieldSize::default();
        let (line, chrome) = size_metrics(size);
        let (min, max, rows, max_rows) = height_bounds(3, 0, size);
        assert_eq!(min, 3.0 * line + chrome, "rows is the min-height floor");
        assert_eq!(max, None, "max_rows == 0 leaves autogrow uncapped");
        assert_eq!((rows, max_rows), (3, 0));
    }

    #[test]
    fn height_bounds_caps_at_max_rows() {
        let size = FieldSize::default();
        let (line, chrome) = size_metrics(size);
        let (min, max, _, max_rows) = height_bounds(2, 8, size);
        assert_eq!(min, 2.0 * line + chrome);
        assert_eq!(max, Some(8.0 * line + chrome), "max_rows is the grow-then-scroll cap");
        assert_eq!(max_rows, 8);
    }

    #[test]
    fn height_bounds_floors_rows_at_one() {
        let size = FieldSize::default();
        let (line, chrome) = size_metrics(size);
        let (min, _, rows, _) = height_bounds(0, 0, size);
        assert_eq!(rows, 1, "a zero-row textarea floors at one line");
        assert_eq!(min, line + chrome);
    }

    #[test]
    fn height_bounds_clamps_cap_below_floor_up_to_floor() {
        // max_rows(2) below rows(4): the cap clamps up to the floor, so
        // the box can't grow past its resting height (min == max) rather
        // than producing an inverted min > max.
        let (min, max, rows, max_rows) = height_bounds(4, 2, FieldSize::default());
        assert_eq!(rows, 4);
        assert_eq!(max_rows, 4);
        assert_eq!(max, Some(min));
    }
}
