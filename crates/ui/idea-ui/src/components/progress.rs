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

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>`. `value` is already `Reactive<f32>` (idempotent skip).
// `tone`/`variant` build the fill's appearance key; `size` the track key —
// both route into their style sinks (`.get()` read INSIDE the closure).
// `indeterminate` selects the WHOLE fill subtree (animated pulse vs value
// track), i.e. structural reactivity — routed through `switch` in `Progress`
// when Dynamic so a live flip swaps the subtree (and frees the prior branch's
// scope-owned animation); Static keeps the direct `if` build.
#[runtime_core::props]
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
            indeterminate: Reactive::Static(false),
            tone: Reactive::Static(ToneRef::default()),
            variant: Reactive::Static(VariantRef::default()),
            size: Reactive::Static(ControlSize::default()),
        }
    }
}

/// Linear progress bar — a muted track with a tone-colored fill.
/// Determinate when `value` is set (fill width tracks it reactively);
/// `indeterminate = true` shows a pulsing full-width fill for
/// unknown-duration work.
#[component]
pub fn Progress(props: &ProgressProps) -> Element {
    let sheets = installed_progress_sheets();

    let fill_sheet = sheets.fill_sheet.clone();
    let track_sheet = sheets.track_sheet.clone();

    // The fill appearance (tone × variant) is read LIVE inside each fill style
    // closure, so the apply-style Effect subscribes to a reactive tone/variant
    // and re-resolves the fill color in place (a bare value just snapshots).
    let appearance_for = {
        let tone = props.tone.clone();
        let variant = props.variant.clone();
        move || format!("{}_{}", tone.get().key(), variant.get().key())
    };

    // The fill is one of TWO subtrees selected by `indeterminate`:
    //   - `true`  → a full-width view whose opacity pulses forever (an
    //     `AnimatedValue` + `av.bind`/`av.animate`, all SCOPE-OWNED — no
    //     `.persist()`/`mem::forget`, so a `switch` rebuild that tears the
    //     branch's nested scope down frees the `Forever` animation atomically).
    //   - `false` → a determinate view whose width tracks `value`.
    // Both subtrees are built INSIDE the component (not passed-in children),
    // so `switch` may rebuild them. Builders are closures so the static path
    // and each switch branch share one definition.
    let build_indeterminate = {
        let fill_sheet = fill_sheet.clone();
        let appearance_for = appearance_for.clone();
        move || -> Element {
            // Full-width fill, opacity pulsing forever. The AnimatedValue and
            // its bind/animate effects are created in THIS scope (the switch
            // branch's scope when dynamic), so they're freed on switch-away.
            let fill_ref: Ref<ViewHandle> = Ref::new();
            let av: AnimatedValue<f32> = AnimatedValue::new(1.0);
            av.bind(fill_ref, AnimProp::Opacity);
            // Prefer a RENDER-SERVER keyframe loop. A forever opacity pulse
            // driven by the per-frame reactive clock forces a full-tree
            // `CA::Transaction::commit` EVERY frame — which measurably steals
            // scroll frames on heavy pages. As a native `CAKeyframeAnimation`
            // (Apple) it costs the main thread nothing per frame. Deferred so the
            // fill `Ref` is mounted before we install on its layer; falls back to
            // the per-frame `AnimatedValue` clock when the backend declines (web,
            // terminal, or a prop with no native keyPath yet).
            let av_fallback = av.clone();
            runtime_core::scheduling::after_ms_scoped(0, move || {
                // One full eased cycle 1.0 → PULSE_MIN → 1.0, repeating forever.
                let keyframes = [(0.0_f32, 1.0_f32), (0.5, PULSE_MIN), (1.0, 1.0)];
                let native = fill_ref
                    .with(|h| {
                        h.install_keyframe_animation(
                            AnimProp::Opacity,
                            &keyframes,
                            (PULSE_MS * 2) as u32,
                            true,
                            false,
                        )
                    })
                    .unwrap_or(false);
                if !native {
                    av_fallback.animate(LoopFactory::new(
                        SequenceFactory::new()
                            .then(
                                TweenTo::new(PULSE_MIN, Duration::from_millis(PULSE_MS))
                                    .ease_in_out(),
                            )
                            .then(
                                TweenTo::new(1.0_f32, Duration::from_millis(PULSE_MS))
                                    .ease_in_out(),
                            ),
                        Repeat::Forever,
                    ));
                }
            });
            let fill_sheet = fill_sheet.clone();
            let appearance_for = appearance_for.clone();
            runtime_core::view(Vec::new())
                .with_style(move || {
                    StyleApplication::new(fill_sheet.clone())
                        .with("appearance", appearance_for())
                        .with_computed("progress-w-100", || StyleRules {
                            width: Some(Tokenized::Literal(Length::pct(100.0))),
                            ..Default::default()
                        })
                })
                .bind(fill_ref)
                .into_element()
        }
    };
    let build_determinate = {
        let fill_sheet = fill_sheet.clone();
        let appearance_for = appearance_for.clone();
        let value = props.value.clone();
        move || -> Element {
            // Determinate — width follows the value, cached per whole percent.
            let fill_sheet = fill_sheet.clone();
            let appearance_for = appearance_for.clone();
            let value = value.clone();
            runtime_core::view(Vec::new())
                .with_style(move || {
                    let pct = (value.get().clamp(0.0, 1.0)) * 100.0;
                    StyleApplication::new(fill_sheet.clone())
                        .with("appearance", appearance_for())
                        .with_computed(format!("progress-w-{}", pct.round() as i32), move || {
                            StyleRules {
                                width: Some(Tokenized::Literal(Length::pct(pct))),
                                ..Default::default()
                            }
                        })
                })
                .into_element()
        }
    };

    // `indeterminate` is STRUCTURAL: it picks the whole fill subtree. Route a
    // Dynamic `indeterminate` through `switch` so a live flip tears down the
    // prior branch's scope (freeing the `Forever` animation when leaving the
    // pulse) and builds the other branch fresh. Static keeps the direct `if`
    // build — no `switch` anchor — mirroring `avatar.rs`'s `src.is_static()`
    // and Field's `style_is_reactive` gate.
    let fill: Element = if props.indeterminate.is_static() {
        if props.indeterminate.get() {
            build_indeterminate()
        } else {
            build_determinate()
        }
    } else {
        let indeterminate = props.indeterminate.clone();
        runtime_core::switch(
            move || indeterminate.get(),
            move |&indet| {
                if indet {
                    build_indeterminate()
                } else {
                    build_determinate()
                }
            },
        )
    };

    // Track thickness follows `size`, read LIVE inside the style closure.
    let track_style = {
        let size = props.size.clone();
        move || {
            StyleApplication::new(track_sheet.clone())
                .with("size", size.get().as_variant_str().to_string())
        }
    };
    let track = runtime_core::view(vec![fill]).with_style(track_style).into_element();
    ui! { view { track } }
}
