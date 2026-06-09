//! `Slider` — a horizontal, draggable value track.
//!
//! A muted rail with a tone-colored fill from the left edge to a round thumb;
//! dragging anywhere on the track sets the value. Controlled: the host owns a
//! `Signal<f32>` and updates it in `on_change`.
//!
//! ```ignore
//! let volume = signal!(0.5_f32);
//! ui! {
//!     Slider(
//!         value = volume,
//!         on_change = move |v: f32| volume.set(v),
//!         tone = tone::Primary,
//!     )
//! }
//! ```
//!
//! ## Drag stability
//!
//! The drag math is `value = x / width`, so the track needs a **known, fixed
//! width** — hence the `width` prop (px, default 184). A percentage/fill width
//! would leave the divisor unknown inside the touch handler. Equally important:
//! the *host* must not re-key or rebuild the Slider mid-drag (e.g. a `when()`
//! keyed on the value being dragged) — that destroys the pointer-capturing view
//! and the drag stalls after the first move. Only the fill/thumb style layers
//! patch as the value changes; the element identity stays stable.

use std::rc::Rc;

use runtime_core::{
    component, AlignItems, Cursor, Element, FlexDirection, IdealystSchema, IntoElement,
    JustifyContent, Length, Position, Reactive, StyleApplication, StyleRules, StyleSheet, Tokenized,
    TouchPhase, TouchResponse, VariantSet,
};

use idea_theme::extensible::{installed_slider_sheets, ToneRef, VariantRef};

use crate::components::ControlSize;

/// Thumb diameter (px) per size — mirrors `SLIDER_DIMS` in idea-theme, used to
/// offset the thumb's `left` so its *center* tracks the value.
fn thumb_diameter(size: ControlSize) -> f32 {
    match size {
        ControlSize::Sm => 12.0,
        ControlSize::Md => 16.0,
        ControlSize::Lg => 20.0,
    }
}

/// Normalized `0..1` position of `v` within `[min, max]`.
fn norm_pos(v: f32, min: f32, max: f32) -> f32 {
    if max <= min {
        0.0
    } else {
        ((v - min) / (max - min)).clamp(0.0, 1.0)
    }
}

#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct SliderProps {
    /// The current value. `Reactive<f32>` — a `Signal<f32>` the host owns, a
    /// static literal, or a model-derived `rx!(...)` getter (so the Slider works
    /// when the value lives in a document/model, read through a closure, not just
    /// a standalone signal). The host applies edits in `on_change`.
    pub value: Reactive<f32>,
    /// Fires with the new value while the user drags.
    pub on_change: Rc<dyn Fn(f32)>,
    /// Lower bound. Default `0.0`.
    pub min: f32,
    /// Upper bound. Default `1.0`.
    pub max: f32,
    /// Snap increment. `0.0` (default) is continuous; `> 0` snaps the value to
    /// the nearest `min + n*step`.
    pub step: f32,
    /// Track width in px. Default `184.0`. A fixed width keeps the drag math
    /// (`x / width`) stable — see the module docs.
    pub width: f32,
    /// Semantic palette for the fill + thumb. Default Primary.
    pub tone: ToneRef,
    /// Surface skeleton for the fill + thumb. Default Filled.
    pub variant: VariantRef,
    /// Rail thickness + thumb size. Default Md.
    pub size: ControlSize,
    /// When `true`, blocks dragging and dims the control. Default `false`.
    pub disabled: bool,
}

impl Default for SliderProps {
    fn default() -> Self {
        Self {
            value: Reactive::Static(0.0),
            on_change: Rc::new(|_| {}),
            min: 0.0,
            max: 1.0,
            step: 0.0,
            width: 184.0,
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            size: ControlSize::default(),
            disabled: false,
        }
    }
}

/// A horizontal draggable value slider — see the module docs.
#[component]
pub fn Slider(props: &SliderProps) -> Element {
    let value = props.value.clone();
    let on_change = props.on_change.clone();
    let (min, max, step) = (props.min, props.max, props.step);
    let w = props.width;
    let size = props.size;
    let disabled = props.disabled;
    let dia = thumb_diameter(size);

    let appearance = format!("{}_{}", props.tone.key(), props.variant.key());
    let size_key = size.as_variant_str().to_string();
    let sheets = installed_slider_sheets();

    // --- fill: width tracks the value%, cached per whole percent ---
    let fill = {
        let fill_sheet = sheets.fill_sheet.clone();
        let app = appearance.clone();
        let value = value.clone();
        runtime_core::view(Vec::new())
            .with_style(move || {
                let pct = norm_pos(value.get(), min, max) * 100.0;
                StyleApplication::new(fill_sheet.clone())
                    .with("appearance", app.clone())
                    .with_computed(format!("slider-fill-{}", pct.round() as i32), move || {
                        StyleRules {
                            width: Some(Tokenized::Literal(Length::pct(pct))),
                            ..Default::default()
                        }
                    })
            })
            .into_element()
    };

    // --- track: the muted rail, holding the fill ---
    let track = {
        let track_sheet = sheets.track_sheet.clone();
        let size_key = size_key.clone();
        runtime_core::view(vec![fill])
            .with_style(move || {
                StyleApplication::new(track_sheet.clone()).with("size", size_key.clone())
            })
            .into_element()
    };

    // --- thumb: round handle, `left` tracks the value (center on the point) ---
    let thumb = {
        let thumb_sheet = sheets.thumb_sheet.clone();
        let app = appearance.clone();
        let size_key = size_key.clone();
        let value = value.clone();
        runtime_core::view(Vec::new())
            .with_style(move || {
                let left = norm_pos(value.get(), min, max) * w - dia / 2.0;
                StyleApplication::new(thumb_sheet.clone())
                    .with("appearance", app.clone())
                    .with("size", size_key.clone())
                    .with_computed(format!("slider-thumb-{}", left.round() as i32), move || {
                        StyleRules {
                            left: Some(Tokenized::Literal(Length::Px(left))),
                            ..Default::default()
                        }
                    })
            })
            .into_element()
    };

    // --- container: fixed-width relative box with the drag handler ---
    let container_style = Rc::new(StyleSheet::new(move |_vs: &VariantSet| StyleRules {
        position: Some(Position::Relative),
        width: Some(Tokenized::Literal(Length::Px(w))),
        height: Some(Tokenized::Literal(Length::Px(dia))),
        flex_direction: Some(FlexDirection::Column),
        justify_content: Some(JustifyContent::Center),
        align_items: Some(AlignItems::Stretch),
        cursor: Some(if disabled { Cursor::Default } else { Cursor::Pointer }),
        opacity: disabled.then(|| Tokenized::Literal(0.45)),
        ..Default::default()
    }));

    runtime_core::view(vec![track, thumb])
        .with_style(container_style)
        .on_touch(move |ev| {
            if disabled {
                return TouchResponse::IGNORED;
            }
            if matches!(ev.phase, TouchPhase::Began | TouchPhase::Moved) {
                let t = (ev.position.x / w).clamp(0.0, 1.0);
                let mut v = min + t * (max - min);
                if step > 0.0 {
                    v = min + ((v - min) / step).round() * step;
                }
                on_change(v.clamp(min, max));
            }
            TouchResponse::CLAIMED
        })
        .into_element()
}
