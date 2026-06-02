//! `Progress` — a linear progress bar. Determinate (a `0.0..=1.0`
//! value) or indeterminate (a pulsing bar for unknown-duration work).
//!
//! ```ignore
//! // Determinate — value can be static or live.
//! ui! { Progress(value = 0.6, tone = tone::Success) }
//! ui! { Progress(value = uploaded /* Signal<f32> */, ) }
//!
//! // Indeterminate.
//! ui! { Progress(indeterminate = true) }
//! ```
//!
//! A muted track with a tone-colored fill. The fill width tracks the
//! value reactively (cached per whole-percent, so a smoothly-changing
//! value mints at most 101 backend classes). Indeterminate mode pulses
//! the full-width fill's opacity via the animator — a uniform,
//! measurement-free indicator that behaves identically on every
//! backend (no sliding bar that would need per-backend width probing).

use std::time::Duration;

use runtime_core::animation::{
    AnimProp, AnimatedValue, LoopFactory, Repeat, SequenceFactory, TweenTo,
};
use runtime_core::{
    component, ui, IdealystSchema, IntoElement, Length, Element, Reactive, Ref, StyleApplication,
    StyleRules, Tokenized, ViewHandle,
};

use idea_theme::extensible::{installed_progress_sheets, ToneRef, VariantRef};

use crate::components::ControlSize;

/// Half-period of the indeterminate opacity pulse.
const PULSE_MS: u64 = 800;
/// Opacity floor of the indeterminate pulse.
const PULSE_MIN: f32 = 0.4;

#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ProgressProps {
    /// Completion in `0.0..=1.0`. Ignored when `indeterminate`.
    /// `Reactive<f32>` — a literal or a live `Signal<f32>` / `rx!`.
    #[schema(constraint = "0.0..=1.0 (clamped)")]
    pub value: Reactive<f32>,
    /// When true, ignore `value` and show a pulsing indeterminate bar.
    pub indeterminate: bool,
    /// Semantic palette for the fill. Default Primary.
    pub tone: ToneRef,
    /// Surface skeleton for the fill. Default Filled.
    pub variant: VariantRef,
    /// Bar thickness. Default Md.
    pub size: ControlSize,
}

impl Default for ProgressProps {
    fn default() -> Self {
        Self {
            value: Reactive::Static(0.0),
            indeterminate: false,
            tone: ToneRef::default(),
            variant: VariantRef::default(),
            size: ControlSize::default(),
        }
    }
}

/// Linear progress bar — a muted track with a tone-colored fill.
/// Determinate when `value` is set (fill width tracks it reactively);
/// `indeterminate = true` shows a pulsing full-width fill for
/// unknown-duration work.
#[component]
pub fn Progress(props: &ProgressProps) -> Element {
    let appearance = format!("{}_{}", props.tone.key(), props.variant.key());
    let size_key = props.size.as_variant_str().to_string();
    let sheets = installed_progress_sheets();

    let fill_sheet = sheets.fill_sheet.clone();
    let track_sheet = sheets.track_sheet.clone();

    let fill: Element = if props.indeterminate {
        // Full-width fill, opacity pulsing forever.
        let fill_ref: Ref<ViewHandle> = Ref::new();
        let av: AnimatedValue<f32> = AnimatedValue::new(1.0);
        av.bind(fill_ref, AnimProp::Opacity);
        av.animate(LoopFactory::new(
            SequenceFactory::new()
                .then(TweenTo::new(PULSE_MIN, Duration::from_millis(PULSE_MS)).ease_in_out())
                .then(TweenTo::new(1.0_f32, Duration::from_millis(PULSE_MS)).ease_in_out()),
            Repeat::Forever,
        ));
        let app = appearance.clone();
        runtime_core::view(Vec::new())
            .with_style(move || {
                StyleApplication::new(fill_sheet.clone())
                    .with("appearance", app.clone())
                    .with_computed("progress-w-100", || StyleRules {
                        width: Some(Tokenized::Literal(Length::pct(100.0))),
                        ..Default::default()
                    })
            })
            .bind(fill_ref)
            .into_element()
    } else {
        // Determinate — width follows the value, cached per whole percent.
        let value = props.value.clone();
        let app = appearance.clone();
        runtime_core::view(Vec::new())
            .with_style(move || {
                let pct = (value.get().clamp(0.0, 1.0)) * 100.0;
                StyleApplication::new(fill_sheet.clone())
                    .with("appearance", app.clone())
                    .with_computed(format!("progress-w-{}", pct.round() as i32), move || {
                        StyleRules {
                            width: Some(Tokenized::Literal(Length::pct(pct))),
                            ..Default::default()
                        }
                    })
            })
            .into_element()
    };

    let track_style = move || StyleApplication::new(track_sheet.clone()).with("size", size_key.clone());
    let track = runtime_core::view(vec![fill]).with_style(track_style).into_element();
    ui! { view { track } }
}
