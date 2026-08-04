//! `Progress` — a linear progress bar with three modes:
//!
//! ```ignore
//! // Value (default) — position follows a `0.0..=1.0` value, static or
//! // live; every change ANIMATES to the new width (sheet transition).
//! ui! { Progress(value = 0.6, tone = tone::Success) }
//! ui! { Progress(value = uploaded /* Signal<f32> */, ) }
//!
//! // Indeterminate — an endless left→right sweep for unknown-duration
//! // work.
//! ui! { Progress(mode = ProgressMode::Indeterminate) }
//!
//! // Simulated — a fake loader: starts empty and creeps toward (but
//! // never reaching) full in irregular steps, as if data were loading.
//! ui! { Progress(mode = ProgressMode::Simulated) }
//! ```
//!
//! A muted track with a tone-colored fill. The value-driven fill width
//! is an INLINE style, so a smoothly-changing value mints no classes at
//! all and the fill sheet still premints (`StyleApplication::with_inline`);
//! the sheet's `mode=determinate` arm carries a width transition so each
//! change glides instead of snapping. The indeterminate sweep is a
//! fixed-fraction segment (`PROGRESS_SWEEP_FRACTION` of the track)
//! translated across the clipped track — the segment's own measured
//! width (via the uniform [`ViewHandle::on_layout`] seam, the same
//! mechanism as Collapsible's measured transition — no per-backend
//! probing) gives the sweep its pixel range on every backend.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use runtime_core::animation::{
    AnimProp, AnimatedValue, LoopFactory, Repeat, SequenceFactory, TweenTo,
};
use runtime_core::{
    component, on_scope_drop, signal, ui, AnchorableHandle, Element, IdealystSchema, IntoElement,
    LayoutSubscription, Length, Reactive, Ref, Signal, StyleApplication, StyleRules, Tokenized,
    ViewHandle,
};

use idea_theme::extensible::{
    installed_progress_sheets, ToneRef, VariantRef, PROGRESS_SWEEP_FRACTION,
};

use crate::components::ControlSize;

/// One full indeterminate sweep (segment enters left, exits right).
const SWEEP_MS: u64 = 1400;

/// Simulated mode parks just short of full — it never "completes" on
/// its own (flip to `Value` with `value = 1.0` when the real work
/// finishes and the fill animates home).
const SIM_CEILING: f32 = 0.92;
/// Jittered bounds on the fraction of remaining headroom a simulated
/// step consumes — later steps are absolutely smaller (geometric
/// decay), matching how real transfers appear to slow near the end.
const SIM_STEP_FRACTION_MIN: f32 = 0.08;
const SIM_STEP_FRACTION_MAX: f32 = 0.30;
/// Jittered bounds on the pause between simulated steps.
const SIM_STEP_MIN_MS: i32 = 350;
const SIM_STEP_MAX_MS: i32 = 1100;
/// xorshift32 seed for the simulated jitter — arbitrary non-zero
/// (the 32-bit golden-ratio constant). Deterministic on purpose: two
/// runs creep identically, which keeps screenshots/tests stable.
const SIM_JITTER_SEED: u32 = 0x9E37_79B9;

/// End-cap treatment for the track + fill. Square (`None`) by default —
/// a progress bar is a measurement surface, and pill ends visually
/// under-report the value at the extremes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema)]
pub enum ProgressCap {
    /// Square ends (default).
    #[default]
    None,
    /// Pill ends on the track and fill.
    Rounded,
}

impl ProgressCap {
    /// The `cap` variant-axis key for this treatment.
    pub fn as_variant_str(self) -> &'static str {
        match self {
            ProgressCap::None => "none",
            ProgressCap::Rounded => "rounded",
        }
    }
}

/// How the bar behaves. Selects the whole fill subtree, so a live
/// change is structural (`switch`) — the outgoing mode's scope-owned
/// animations and timers are freed atomically on the flip.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default, IdealystSchema)]
pub enum ProgressMode {
    /// Position follows the `value` prop; every change animates to the
    /// new width via the fill sheet's width transition.
    #[default]
    Value,
    /// An endless left→right sweep for unknown-duration work.
    Indeterminate,
    /// A fake loader: creeps from empty toward (never reaching) full
    /// in irregular, shrinking steps — as if data were loading.
    /// Ignores `value`.
    Simulated,
}

// Reactive-by-default: `#[props]` wraps each scalar-DATA field `T` →
// `Reactive<T>`. `value` is already `Reactive<f32>` (idempotent skip).
// `tone`/`variant` build the fill's appearance key; `size` the track key —
// both route into their style sinks (`.get()` read INSIDE the closure).
// `mode` selects the WHOLE fill subtree, i.e. structural reactivity —
// routed through `switch` in `Progress` when Dynamic so a live flip swaps
// the subtree (and frees the prior branch's scope-owned animation/timers);
// Static keeps the direct build.
#[runtime_core::props]
#[derive(IdealystSchema)]
#[cfg_attr(feature = "docs", derive(idea_ui::doc_controls::DocControls))]
pub struct ProgressProps {
    /// Completion in `0.0..=1.0`. Read in [`ProgressMode::Value`];
    /// ignored by the other modes. `Reactive<f32>` — a literal or a
    /// live `Signal<f32>` / `rx!`.
    #[schema(constraint = "0.0..=1.0 (clamped)")]
    pub value: Reactive<f32>,
    /// Behavior: value-driven (default), endless indeterminate sweep,
    /// or simulated loading creep.
    pub mode: Reactive<ProgressMode>,
    /// Semantic palette for the fill. Default Primary.
    pub tone: ToneRef,
    /// Surface skeleton for the fill. Default Filled.
    pub variant: VariantRef,
    /// Bar thickness. Default Md.
    pub size: ControlSize,
    /// End caps: square (default) or rounded (pill).
    pub cap: ProgressCap,
}

impl Default for ProgressProps {
    fn default() -> Self {
        Self {
            value: Reactive::Static(0.0),
            mode: Reactive::Static(ProgressMode::Value),
            tone: Reactive::Static(ToneRef::default()),
            variant: Reactive::Static(VariantRef::default()),
            size: Reactive::Static(ControlSize::default()),
            cap: Reactive::Static(ProgressCap::default()),
        }
    }
}

/// xorshift32 step → uniform `0.0..1.0`. Plain deterministic PRNG —
/// the simulated creep only needs "looks irregular", not entropy.
fn sim_rand01(state: &Cell<u32>) -> f32 {
    let mut x = state.get();
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    state.set(x);
    // Top 24 bits → exact in f32, uniform in [0, 1).
    (x >> 8) as f32 / (1u32 << 24) as f32
}

/// One simulated step: consume a jittered fraction of the headroom
/// left below [`SIM_CEILING`]. Monotonic, bounded, geometrically
/// decaying — pure so the creep curve is unit-testable.
fn sim_next(cur: f32, rand01: f32) -> f32 {
    let fraction =
        SIM_STEP_FRACTION_MIN + (SIM_STEP_FRACTION_MAX - SIM_STEP_FRACTION_MIN) * rand01;
    (cur + (SIM_CEILING - cur) * fraction).min(SIM_CEILING)
}

/// Arm the next simulated creep step. `after_ms_scoped` is the
/// TOP-LEVEL `runtime_core` export — the newcore World-anchored one
/// from `runtime_vocabulary::scoped_scheduling`. (Its old-arena
/// predecessor, formerly reachable as
/// `runtime_core::scheduling::after_ms_scoped`, re-entered only the
/// pre-World reactive ctx — a world-signal timer chain registered
/// through it silently died; it's `pub(crate)` in runtime-shared now,
/// so the wrong choice no longer compiles.) The callback re-enters
/// the registering anchor, so the recursive re-arm chains in the SAME
/// scope and the whole chain dies with it (a mode flip or unmount
/// drops the pending shot — no detached timers).
fn schedule_sim_step(progress: Signal<f32>, rng: Rc<Cell<u32>>) {
    let delay = SIM_STEP_MIN_MS
        + ((SIM_STEP_MAX_MS - SIM_STEP_MIN_MS) as f32 * sim_rand01(&rng)) as i32;
    runtime_core::after_ms_scoped(delay, move || {
        let r = sim_rand01(&rng);
        progress.set(sim_next(progress.get(), r));
        schedule_sim_step(progress, rng);
    });
}

/// Linear progress bar — a muted track with a tone-colored fill.
/// `mode` picks value-driven (default, animated width), an endless
/// indeterminate sweep, or a simulated loading creep.
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
    // End-cap axis, shared by the track and every fill flavor (read
    // LIVE inside each style closure, same contract as `appearance`).
    let cap_for = {
        let cap = props.cap.clone();
        move || cap.get().as_variant_str().to_string()
    };

    // A determinate-looking fill whose width (0..=1 fraction) comes from
    // `fraction_source` — shared by the `Value` mode (reads the prop) and
    // the `Simulated` mode (reads its private creep signal). The sheet's
    // `mode=determinate` arm carries the width transition, so every
    // source change ANIMATES to the new width on all backends.
    let build_valued_fill = {
        let fill_sheet = fill_sheet.clone();
        let appearance_for = appearance_for.clone();
        let cap_for = cap_for.clone();
        move |fraction_source: Rc<dyn Fn() -> f32>| -> Element {
            let fill_sheet = fill_sheet.clone();
            let appearance_for = appearance_for.clone();
            let cap_for = cap_for.clone();
            runtime_core::view(Vec::new())
                .with_style(move || {
                    let pct = runtime_core::num::clamp_f32(fraction_source(), 0.0, 1.0) * 100.0;
                    StyleApplication::new(fill_sheet.clone())
                        .with("appearance", appearance_for())
                        .with("cap", cap_for())
                        .with("mode", "determinate".to_string())
                        // Continuous: a computed layer keyed on the rounded
                        // percent minted a cache entry and a CSS class per
                        // whole percent. Inline keeps the fill sheet
                        // premintable and puts only the width on the node.
                        .with_inline(StyleRules {
                            width: Some(Tokenized::Literal(Length::pct(pct))),
                            ..Default::default()
                        })
                })
                .into_element()
        }
    };

    // The fill is one of THREE subtrees selected by `mode`. All are built
    // INSIDE the component (not passed-in children) so `switch` may rebuild
    // them; builders are closures so the static path and each switch branch
    // share one definition. Everything each branch allocates (AnimatedValue
    // binds, layout subscriptions, creep timers) is SCOPE-OWNED — no
    // `.persist()`/`mem::forget` — so a switch-away frees it atomically.
    let build_value = {
        let build_valued_fill = build_valued_fill.clone();
        let value = props.value.clone();
        move || -> Element {
            let value = value.clone();
            build_valued_fill(Rc::new(move || value.get()))
        }
    };
    let build_simulated = {
        let build_valued_fill = build_valued_fill.clone();
        move || -> Element {
            // Private creep state, reset every time the branch (re)builds —
            // entering Simulated always starts from empty.
            let progress: Signal<f32> = signal(0.0);
            schedule_sim_step(progress, Rc::new(Cell::new(SIM_JITTER_SEED)));
            build_valued_fill(Rc::new(move || progress.get()))
        }
    };
    let build_indeterminate = {
        let fill_sheet = fill_sheet.clone();
        let appearance_for = appearance_for.clone();
        let cap_for = cap_for.clone();
        move || -> Element {
            // A sweep segment (constant `PROGRESS_SWEEP_FRACTION` of the
            // track, from the sheet's `mode=indeterminate` arm) translated
            // across the overflow-clipped track forever.
            //
            // The pixel range comes from the segment's own measured width
            // `w` (uniform `on_layout` seam): track width is `w / fraction`,
            // so the loop runs `-w → w / fraction` — fully hidden left to
            // fully exited right. No per-backend probing (CLAUDE.md §7).
            let fill_ref: Ref<ViewHandle> = Ref::new();
            let fill_w: Signal<f32> = signal(0.0);
            let av: AnimatedValue<f32> = AnimatedValue::new(0.0);
            av.bind(fill_ref, AnimProp::TranslateX);

            // Measure the fill. Deferred to `after_animation_frame` because
            // the Ref fills only after the mount pass; the subscription is
            // anchored to THIS scope via `on_scope_drop` (never leaked —
            // a late ResizeObserver callback into a freed signal aborts;
            // see Collapsible's measured body, the reference consumer).
            //
            // Two measurement sources, deliberately: `rect()` seeds the
            // initial width on EVERY backend that can measure (the GPU
            // backend has `rect` but no layout subscription yet), while
            // `on_layout` keeps the range live across resizes where it's
            // wired (web / iOS / Android / macOS).
            let sub_holder: Rc<RefCell<Option<LayoutSubscription>>> =
                Rc::new(RefCell::new(None));
            let holder_for_setup = sub_holder.clone();
            let fill_ref_for_setup = fill_ref;
            let setup_task = runtime_core::after_animation_frame(move || {
                let seed_w = fill_ref_for_setup.with(|h| h.rect().width).unwrap_or(0.0);
                if seed_w > 0.5 {
                    fill_w.set(seed_w);
                }
                let sub_opt = fill_ref_for_setup.with(|h| {
                    h.on_layout(move |w, _h| {
                        if (fill_w.get() - w).abs() > 0.5 {
                            fill_w.set(w);
                        }
                    })
                });
                if let Some(sub) = sub_opt {
                    *holder_for_setup.borrow_mut() = Some(sub);
                }
            });
            on_scope_drop(move || {
                drop(setup_task);
                drop(sub_holder);
            });

            // (Re)start the sweep whenever the measured width changes —
            // first layout starts it, a resize restarts it with the new
            // pixel range (the loop resets to the left edge; resizes are
            // rare enough that the blip doesn't matter).
            let av_for_sweep = av.clone();
            runtime_core::effect!({
                let w = fill_w.get();
                if w > 0.0 {
                    let start = -w;
                    let end = w / PROGRESS_SWEEP_FRACTION;
                    // Prefer a RENDER-SERVER keyframe loop: a forever
                    // per-frame animation forces a full-tree commit every
                    // frame on Apple (measurably steals scroll frames —
                    // the same rationale as the old opacity pulse's native
                    // path). Falls back to the per-frame AnimatedValue
                    // clock when the backend has no native keyPath for
                    // the prop.
                    let native = fill_ref
                        .with(|h| {
                            h.install_keyframe_animation(
                                AnimProp::TranslateX,
                                &[(0.0, start), (1.0, end)],
                                SWEEP_MS as u32,
                                true,
                                false,
                            )
                        })
                        .unwrap_or(false);
                    if !native {
                        av_for_sweep.set(start);
                        av_for_sweep.animate(LoopFactory::new(
                            SequenceFactory::new()
                                .then(
                                    TweenTo::new(end, Duration::from_millis(SWEEP_MS))
                                        .ease_in_out(),
                                )
                                // Zero-duration tween = snap back to the left
                                // edge for the next pass.
                                .then(TweenTo::new(start, Duration::ZERO)),
                            Repeat::Forever,
                        ));
                    }
                }
            });

            let fill_sheet = fill_sheet.clone();
            let appearance_for = appearance_for.clone();
            let cap_for = cap_for.clone();
            runtime_core::view(Vec::new())
                .with_style(move || {
                    StyleApplication::new(fill_sheet.clone())
                        .with("appearance", appearance_for())
                        .with("cap", cap_for())
                        .with("mode", "indeterminate".to_string())
                })
                .bind(fill_ref)
                .into_element()
        }
    };

    let build_for = move |mode: ProgressMode| -> Element {
        match mode {
            ProgressMode::Value => build_value(),
            ProgressMode::Indeterminate => build_indeterminate(),
            ProgressMode::Simulated => build_simulated(),
        }
    };

    // `mode` is STRUCTURAL: it picks the whole fill subtree. Route a Dynamic
    // `mode` through `switch` so a live flip tears down the prior branch's
    // scope (freeing its sweep animation / creep timers) and builds the next
    // branch fresh. Static keeps the direct build — no `switch` anchor —
    // mirroring `avatar.rs`'s `src.is_static()` gate.
    let fill: Element = if props.mode.is_static() {
        build_for(props.mode.get())
    } else {
        let mode = props.mode.clone();
        runtime_core::switch(move || mode.get(), move |&m| build_for(m))
    };

    // Track thickness follows `size`, caps follow `cap` — both read
    // LIVE inside the style closure.
    let track_style = {
        let size = props.size.clone();
        move || {
            StyleApplication::new(track_sheet.clone())
                .with("size", size.get().as_variant_str().to_string())
                .with("cap", cap_for())
        }
    };
    let track = runtime_core::view(vec![fill]).with_style(track_style).into_element();
    ui! { view { track } }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The creep is monotonic, stays below its ceiling forever, and its
    /// absolute step size decays (later steps are smaller than the
    /// first) — the "mimics a real load" contract of Simulated mode.
    #[test]
    fn simulated_creep_is_monotonic_bounded_and_decaying() {
        let rng = Cell::new(SIM_JITTER_SEED);
        let first = sim_next(0.0, sim_rand01(&rng));
        assert!(first > 0.0, "the first step moves off zero");
        let mut last = first;
        for _ in 0..500 {
            let next = sim_next(last, sim_rand01(&rng));
            assert!(next >= last, "creep never moves backwards");
            assert!(next <= SIM_CEILING, "creep never exceeds its ceiling");
            last = next;
        }
        // Geometric decay: with almost no headroom left, even the
        // largest allowed step is smaller than the first one.
        let late_step_max = (SIM_CEILING - last) * SIM_STEP_FRACTION_MAX;
        assert!(
            late_step_max < first,
            "steps shrink as the bar approaches the ceiling"
        );
    }

    /// xorshift32 mapped through the top-24-bit divide stays in [0, 1).
    #[test]
    fn sim_rand01_stays_in_unit_range() {
        let rng = Cell::new(SIM_JITTER_SEED);
        for _ in 0..10_000 {
            let r = sim_rand01(&rng);
            assert!((0.0..1.0).contains(&r), "rand01 out of range: {r}");
        }
    }
}
