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
    /// Pin an exact minimum height in pixels, overriding the `rows`-derived
    /// floor. Use this to match a design's exact px min-height (e.g.
    /// `min_height = Some(92.0)`) instead of guessing a `rows` count.
    /// `None` (the default) keeps the `rows`/`size`-derived floor.
    #[schema(constraint = "pixels; None = rows-derived floor")]
    pub min_height: Option<f32>,
    /// Pin an exact width in pixels. `None` (the default) lets the input
    /// fill its column; `Some(px)` fixes the width.
    #[schema(constraint = "pixels; None = fill column")]
    pub width: Option<f32>,
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
            min_height: None,
            width: None,
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
    let appearance = props.variant.as_variant_str().to_string();
    let size_key = size.as_variant_str().to_string();

    // The effective tone is reactive iff it's *derived* from a live error
    // signal — no explicit tone given AND `error` is `Dynamic`. (Mirrors
    // Field; see the `INVARIANT (D9)` note below.)
    let explicit_tone = props.tone.clone();
    let error = props.error.clone();
    let tone_is_reactive = explicit_tone.is_none() && !error.is_static();
    let tone_key_for = {
        let explicit_tone = explicit_tone.clone();
        let error = error.clone();
        move || -> String {
            let tone: Option<ToneRef> = explicit_tone.clone().or_else(|| {
                if error.get().is_some() { Some(tones::Danger.into()) } else { None }
            });
            tone.as_ref().map(|t| t.key()).unwrap_or("default").to_string()
        }
    };

    let (rows_min_height, max_height, _rows, _max_rows) =
        height_bounds(props.rows, props.max_rows, size);
    // An explicit `min_height` prop overrides the rows-derived floor; else
    // keep the `rows`/`size`-derived height as the default.
    let min_height = props.min_height.unwrap_or(rows_min_height);
    let width = props.width;
    // Key the computed dim layer by the *resolved* px values so a textarea
    // pinned via `min_height` dedupes with the same-config siblings.
    let dim_key = format!(
        "ta-dim-{}-{:?}-{:?}-{}",
        min_height, max_height, width, size_key
    );

    // STATIC style (reuse Field's sheets) + a computed min/max-height +
    // width layer keyed so identical configs dedupe to one class. The
    // primitive sizes the box to its content; these bounds set the resting
    // floor and the grow-then-scroll cap.
    let make_input_style = {
        let appearance = appearance.clone();
        let size_key = size_key.clone();
        move |tone_key: String| -> StyleApplication {
            let dim_key = dim_key.clone();
            StyleApplication::new(field_input_sheet())
                .with("size", size_key.clone())
                .with("appearance", appearance.clone())
                .with("tone", tone_key)
                .with_computed(dim_key, move || StyleRules {
                    min_height: Some(Tokenized::Literal(Length::Px(min_height))),
                    max_height: max_height.map(|h| Tokenized::Literal(Length::Px(h))),
                    width: width.map(|w| Tokenized::Literal(Length::Px(w))),
                    ..Default::default()
                })
        }
    };

    let help_style =
        StyleApplication::new(field_help_sheet()).with("tone", tone_key_for());

    let label_node = crate::components::optional_reactive_text(props.label.clone(), FieldLabel());

    let help_combined = match (props.error.clone(), props.help.clone()) {
        (Reactive::Static(e), Reactive::Static(h)) => Reactive::Static(e.or(h)),
        (e, h) => Reactive::Dynamic(Rc::new(move || e.get().or_else(|| h.get()))),
    };
    let help_node = crate::components::optional_reactive_text(help_combined, help_style);

    let mut input = runtime_core::text_area(value, move |v: String| (on_change)(v));
    if let Some(p) = placeholder {
        input = input.placeholder(p);
    }
    // INVARIANT (D9): see Field. A live-error-derived border MUST be a
    // reactive style closure (re-reads `error.get()` inside the apply
    // Effect) or it snapshots the border color at build time — only the
    // error TEXT would update on validation, not the border. Static fast
    // path stays when the tone is fixed.
    let input = if tone_is_reactive {
        let make_input_style = make_input_style.clone();
        let tone_key_for = tone_key_for.clone();
        input.with_style(move || make_input_style(tone_key_for()))
    } else {
        input.with_style(make_input_style(tone_key_for()))
    };
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

    use idea_theme::theme::{install_idea_theme, light_theme};
    use runtime_core::{resolve_style, StyleSource};

    /// Pull the `StyleSource` off the `text_area` node inside a built
    /// `Textarea` element tree.
    fn input_style_source(ta: Element) -> StyleSource {
        let children = match ta {
            Element::View { children, .. } => children,
            _ => panic!("Textarea renders a view wrapper"),
        };
        for c in children {
            if let Element::TextArea { style, .. } = c {
                return style.expect("Textarea's text_area always has a style");
            }
        }
        panic!("Textarea tree has no text_area node");
    }

    // D9 regression (mirror of Field): a live `error` signal must drive
    // the border color reactively, not snapshot it at build.
    #[test]
    fn reactive_error_drives_border_color_live() {
        install_idea_theme(light_theme());
        let err: Signal<Option<String>> = Signal::new(None);
        let props = TextareaProps {
            error: err.into(),
            ..Default::default()
        };
        let closure = match input_style_source(Textarea(&props)) {
            StyleSource::Reactive(f) => f,
            _ => panic!(
                "a Textarea with a reactive `error` must attach a reactive style \
                 source (D9 regression)"
            ),
        };
        let border_none = resolve_style(&closure()).border_top_color.clone();
        err.set(Some("Required".into()));
        let border_err = resolve_style(&closure()).border_top_color.clone();
        assert!(border_none.is_some() && border_err.is_some());
        assert_ne!(
            border_none, border_err,
            "flipping the error signal must change the border color"
        );
    }

    #[test]
    fn fixed_tone_uses_static_style_source() {
        install_idea_theme(light_theme());
        let props = TextareaProps {
            error: Reactive::Static(Some("bad".into())),
            ..Default::default()
        };
        assert!(matches!(
            input_style_source(Textarea(&props)),
            StyleSource::Static(_)
        ));
    }

    // D6: an explicit `min_height` prop overrides the rows-derived floor.
    #[test]
    fn min_height_prop_overrides_rows_floor() {
        install_idea_theme(light_theme());
        let props = TextareaProps {
            // rows would derive a different floor; the prop wins.
            rows: 3,
            min_height: Some(92.0),
            ..Default::default()
        };
        let rules = match input_style_source(Textarea(&props)) {
            StyleSource::Static(app) => resolve_style(&app),
            _ => panic!("no reactive error → static"),
        };
        assert_eq!(
            rules.min_height,
            Some(Tokenized::Literal(Length::Px(92.0))),
            "min_height prop pins the exact px floor, overriding rows"
        );
    }

    // D6: width prop pins an exact px width.
    #[test]
    fn width_prop_sets_width_style() {
        install_idea_theme(light_theme());
        let props = TextareaProps {
            width: Some(320.0),
            ..Default::default()
        };
        let rules = match input_style_source(Textarea(&props)) {
            StyleSource::Static(app) => resolve_style(&app),
            _ => unreachable!(),
        };
        assert_eq!(rules.width, Some(Tokenized::Literal(Length::Px(320.0))));
    }
}
