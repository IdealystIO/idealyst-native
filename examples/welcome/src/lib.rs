//! `welcome` — three-act cinematic intro, driven by the framework's
//! animation system (springs + tweens) rather than `Presence`'s
//! enter/exit transitions.
//!
//! Act 1 — "Welcome to Idealyst" rises into a light frame, settles.
//! Act 2 — The frame washes to dark; a warm sun-glare blooms in
//!         from the top-right corner; the welcome phrase exits.
//! Act 3 — Content scales from oversized down to rest, reading as
//!         "focus pulling in."
//!
//! Each animated property is its own `AnimatedValue<f32>` bound to
//! a `Ref<ViewHandle>` via `subscribe_and_apply`. Per-frame the
//! subscriber writes `opacity` / `transform: translate(...) scale(...)`
//! as inline CSS via `web::set_animated_f32`. The act-sequence
//! `effect!` fires `av.animate(SpringTo::new(...))` (or `TweenTo`)
//! calls at the right moments — that's the whole orchestration.
//!
//! Springs (`SpringTo::new(target).stiffness(s).damping(d)`) carry
//! the entrances; tweens with cubic-bezier easing carry fades and
//! exits.
//!
//! `app()` is invoked via `framework_core::mount(backend, super::app)`
//! (see `src/web.rs`) so the `effect!` below adopts the root scope.

#[cfg(target_arch = "wasm32")]
mod web;

use std::rc::Rc;
use std::time::Duration;

use framework_core::animation::{AnimProp, AnimatedValue, SpringTo, TweenTo};
use framework_core::{
    animated, effect, node_ref, on_cleanup, timeline, ui, AlignItems, Color, FlexDirection,
    FontWeight, JustifyContent, Length, Position, Primitive, Ref, StyleRules, StyleSheet,
    TextAlign, TextHandle, Tokenized, ViewHandle,
};

// ---- Timing (milliseconds) ----------------------------------------------

/// Pause after page load before Act 1's phrase begins entering.
const INTRO_PAUSE_MS: i32 = 400;

/// Act 1 hold — how long the welcome phrase sits at rest before
/// the dark wash begins.
const ACT_1_HOLD_MS: i32 = 1700;

/// Welcome phrase enter — how long the spring takes to roughly
/// settle (springs don't have a hard duration; this is the lag we
/// schedule against before Act 2 begins).
const PHRASE_ENTER_BUDGET_MS: i32 = 900;

/// Dark wash duration. Slow, deliberate.
const DARK_FADE_MS: u64 = 1300;

/// How long after the sun-glare starts blooming the content arrives.
/// The content enters *during* the glare's bloom so the scene
/// transformation (welcome out → dark + glare in → content in) reads
/// as one composed motion rather than three sequential beats.
///
/// Tuned by ear: long enough that the welcome phrase is well past
/// gone before content lands; short enough that the glare and
/// content are visibly arriving together.
const CONTENT_OFFSET_AFTER_GLARE_MS: i32 = 600;

// ---- Motion / scale constants -------------------------------------------

/// Initial rise distance for the welcome phrase, in CSS pixels.
const PHRASE_ENTER_Y: f32 = 24.0;

/// How far the welcome phrase shuffles UP in Act 3 to make room
/// for the subtitle. Just enough that the subtitle reads as "new
/// information appearing below," not "second line of a paragraph."
const WELCOME_SHUFFLE_Y: f32 = -28.0;

/// Initial scale of the welcome phrase — slightly under 1.0 so the
/// spring eases up into the resting size with a touch of bounce.
const PHRASE_ENTER_SCALE: f32 = 0.95;

/// Initial rise distance for the subtitle. Small — its main motion
/// is the fade-in; the small slide adds character without competing
/// with the welcome's shuffle.
const SUBTITLE_ENTER_Y: f32 = 10.0;

/// Sun-glare bloom — starts small (a tight point of light) and
/// scales up to its resting size. A loose spring (low stiffness,
/// moderate damping) gives the bloom a slow, organic spread.
const GLARE_INITIAL_SCALE: f32 = 0.55;

/// Sun-glare anchor size as a fraction of viewport width. The
/// stylesheet pairs this with `aspect_ratio: 1.0` so the box is
/// always square regardless of viewport aspect — the layout engine
/// derives height to match. `40%` of a 390-wide iPhone ≈ 156px;
/// `40%` of a 1024-wide iPad ≈ 410px.
const GLARE_ANCHOR_SIZE_PCT: f32 = 60.0;

/// Negative-margin shift that centers the anchor on the top-right
/// viewport corner. Both `margin_top` and `margin_right` percentages
/// resolve against the parent's INLINE size (= width) per CSS
/// spec, so a single value works for both axes regardless of
/// device aspect ratio — half the anchor's width-as-percent shifts
/// the box up by half its (square) height AND right by half its
/// width, putting the box center exactly on the corner.
const GLARE_CENTER_SHIFT_PCT: f32 = -(GLARE_ANCHOR_SIZE_PCT / 2.0);

/// Sun-glare breathe amplitude. The raf-driven pulse adds
/// `sin(t) * amp` to the resting scale of 1.0, so the sun throbs
/// between `1 - amp` and `1 + amp`. ±8% reads as a clear, organic
/// breath; larger feels gimmicky, smaller is invisible.
const SUN_PULSE_AMPLITUDE: f32 = 0.08;

/// Period of the sun's color + scale breathe, in milliseconds. Both
/// the per-stop color animator and the scale animator share this so
/// the warmth swell and the size swell stay phase-locked. ~5 s reads
/// as an unhurried, alive presence; faster starts to feel anxious.
const SUN_PULSE_PERIOD_MS: f64 = 5200.0;


// ---- Color palette -------------------------------------------------------

const COLOR_LIGHT_BG: &str = "#f7f5ef";
const COLOR_DARK_BG: &str = "#0a0c11";
const COLOR_HEADLINE_DARK: &str = "#0a0c11";
const COLOR_HEADLINE_LIGHT: &str = "#f4ead8";
const COLOR_SUBTITLE_LIGHT: &str = "#a89a7d";
/// Sun-glare core — near-white with the faintest warmth.
const COLOR_SUN_CORE: &str = "#fff6d8";

// ---- Pulse palette -------------------------------------------------------
//
// Two-color cycles for the raf-driven pulse. Each pair is `(dim,
// bright)` — `sin(t)` maps `0..1` between them. Sticking with the
// warm-gold family on both ends keeps the pulse breathing rather
// than oscillating between two visibly different hues. Channels
// are 0..=1 sRGB; alpha is independent.
const SUN_CORE_DIM:        (f32, f32, f32, f32) = (1.0, 0.95, 0.78, 0.95);
const SUN_CORE_BRIGHT:     (f32, f32, f32, f32) = (1.0, 0.99, 0.90, 1.00);
const SUN_CORONA_DIM:      (f32, f32, f32, f32) = (1.0, 0.78, 0.36, 0.70);
const SUN_CORONA_BRIGHT:   (f32, f32, f32, f32) = (1.0, 0.85, 0.50, 0.95);
// Alpha range: the vignette is supposed to read as ambient
// warmth at the very edge of the frame. Each edge band peaks at
// these alphas; where two bands overlap in a corner the effective
// alpha doubles.
const VIGNETTE_CORNER_DIM:    (f32, f32, f32, f32) = (1.0, 0.78, 0.36, 0.015);
const VIGNETTE_CORNER_BRIGHT: (f32, f32, f32, f32) = (1.0, 0.85, 0.50, 0.06);

// ---- Typography sizes ----------------------------------------------------

const HEADLINE_SIZE_PX: f32 = 56.0;
const SUBTITLE_SIZE_PX: f32 = 18.0;

pub fn app() -> Primitive {
    // ---- Animated values -----------------------------------------------
    //
    // One AV per (element, property) pair. Each AV holds a current
    // value + velocity; `animate(...)` retargets, the clock ticks,
    // subscribers write the new value to the bound element each
    // frame. Springs and tweens compose freely on the same value
    // (e.g. a spring can be retargeted mid-flight to a tween).

    let welcome_opacity = animated!(0.0_f32);
    let welcome_scale = animated!(PHRASE_ENTER_SCALE);
    let welcome_y = animated!(PHRASE_ENTER_Y);
    // Foreground color of the welcome phrase. Starts at the dark
    // ink color (matches the light background); animates to the
    // light cream color during Act 2 so the phrase stays readable
    // once the background goes dark.
    let welcome_color = animated!(srgb_tuple(COLOR_HEADLINE_DARK));

    let dark_opacity = animated!(0.0_f32);

    let glare_opacity = animated!(0.0_f32);
    let glare_scale = animated!(GLARE_INITIAL_SCALE);
    // The sun's gradient stops are animatable per-frame via
    // `AnimProp::GradientStopColor(idx)`. Seed each AV at the same
    // color the stylesheet uses so the per-frame writes pick up
    // exactly where the static apply leaves off — `start_pulse`
    // in Act 2 retargets them with a sine-driven oscillation.
    let sun_core_color = animated!(srgb_tuple(COLOR_SUN_CORE));
    let sun_corona_color = animated!(srgba_tuple(255.0, 210.0, 110.0, 0.85));

    // Vignette — full-page radial gradient that ambient-lights the
    // frame in warm yellow during the dark phase. Center is fully
    // transparent so it doesn't wash out the welcome text; corners
    // glow `COLOR_SUN_GLOW`-warm and pulse with the sun.
    let vignette_opacity = animated!(0.0_f32);
    // Stop 2 is the corner color (stops 0 + 1 stay transparent).
    let vignette_corner_color = animated!(srgba_tuple(255.0, 168.0, 60.0, 0.0));

    let subtitle_opacity = animated!(0.0_f32);
    let subtitle_y = animated!(SUBTITLE_ENTER_Y);

    // ---- Refs ----------------------------------------------------------
    //
    // `welcome_ref` (wrapper View) carries opacity / scale /
    // translate — those properties cascade through UIView's alpha
    // and transform to the Text child. Color animation can't ride
    // that cascade (UILabel.textColor is independent of its
    // parent's tintColor), so a separate `welcome_text_ref` is
    // bound to the Text itself, and the color AV writes to it
    // through `drive_color_text_av`.

    let welcome_ref = node_ref!(ViewHandle);
    let welcome_text_ref = node_ref!(TextHandle);
    let dark_ref = node_ref!(ViewHandle);
    let vignette_ref = node_ref!(ViewHandle);
    // Four edge bands give us a rounded-rectangle vignette without
    // the elliptical center artefact a radial gradient produces on a
    // non-square viewport. Each band is a linear gradient from
    // its inner edge (transparent) to the screen edge (warm); where
    // two bands overlap in a corner, their alphas add, so corners
    // come out a touch brighter — which reads correctly as "the
    // light is strongest at the corners."
    let vignette_top_ref = node_ref!(ViewHandle);
    let vignette_bottom_ref = node_ref!(ViewHandle);
    let vignette_left_ref = node_ref!(ViewHandle);
    let vignette_right_ref = node_ref!(ViewHandle);
    let glare_ref = node_ref!(ViewHandle);
    let subtitle_ref = node_ref!(ViewHandle);

    // Wire each AV to its target node + property. After this,
    // every `animate(...)` call automatically writes per-frame
    // values into the bound element.
    drive_av(&welcome_opacity, welcome_ref, AnimProp::Opacity);
    drive_av(&welcome_scale, welcome_ref, AnimProp::Scale);
    drive_av(&welcome_y, welcome_ref, AnimProp::TranslateY);
    drive_color_text_av(&welcome_color, welcome_text_ref, AnimProp::ForegroundColor);
    drive_av(&dark_opacity, dark_ref, AnimProp::Opacity);
    drive_av(&vignette_opacity, vignette_ref, AnimProp::Opacity);
    drive_av(&glare_opacity, glare_ref, AnimProp::Opacity);
    drive_av(&glare_scale, glare_ref, AnimProp::Scale);
    // Per-stop color animation. The sun has 4 stops: core (0) →
    // corona (1) → halo (2) → transparent (3); we pulse stops 0 + 1.
    // The vignette has 3 stops: transparent (0) → transparent (1) →
    // warm corner (2); we pulse stop 2.
    drive_gradient_stop_av(&sun_core_color, glare_ref, 0);
    drive_gradient_stop_av(&sun_corona_color, glare_ref, 1);
    // Each edge band's gradient is `[transparent at 0, warm at 1]`
    // with the angle pointing the warm stop AT the screen edge.
    // We drive index 1 on all four refs with the same AV so the
    // bands breathe in lock-step.
    drive_gradient_stop_av(&vignette_corner_color, vignette_top_ref, 1);
    drive_gradient_stop_av(&vignette_corner_color, vignette_bottom_ref, 1);
    drive_gradient_stop_av(&vignette_corner_color, vignette_left_ref, 1);
    drive_gradient_stop_av(&vignette_corner_color, vignette_right_ref, 1);
    drive_av(&subtitle_opacity, subtitle_ref, AnimProp::Opacity);
    drive_av(&subtitle_y, subtitle_ref, AnimProp::TranslateY);

    // ---- Schedule the timeline -----------------------------------------
    //
    // Three acts, each fires a handful of `animate(...)` calls
    // simultaneously to drive the per-property motion. Springs do
    // most of the heavy lifting; tweens carry the longer fades.

    let act_1_start = INTRO_PAUSE_MS;
    let act_2_start = act_1_start + PHRASE_ENTER_BUDGET_MS + ACT_1_HOLD_MS;
    // The glare itself starts 200ms after Act 2 begins (the lag
    // built into its scheduling below). Content enters
    // `CONTENT_OFFSET_AFTER_GLARE_MS` later — so glare and content
    // bloom in parallel rather than sequentially.
    let act_3_start = act_2_start + 200 + CONTENT_OFFSET_AFTER_GLARE_MS;

    effect!({
        // The whole cinematic timeline in one declarative block.
        // Each `t => { ... }` clause fires all its `av: animator`
        // pairs at moment `t`. Tasks are collected into a Vec
        // kept alive by `on_cleanup` for the page lifetime.
        let tasks = timeline! {
            // ── Act 1 — welcome phrase enters with a settle spring.
            act_1_start => {
                welcome_opacity: TweenTo::new(1.0, Duration::from_millis(700)).ease_out(),
                welcome_scale: SpringTo::new(1.0).stiffness(170.0).damping(22.0),
                welcome_y: SpringTo::new(0.0).stiffness(170.0).damping(22.0),
            },
            // ── Act 2 — wash to dark, sun-glare blooms in. The
            // welcome phrase stays mounted; only its color animates
            // (dark ink → light cream) so the phrase remains
            // readable as the background swings under it.
            // ── Act 2 — wash to dark + vignette glow comes up + sun
            // begins blooming. Once everything's mounted, the
            // raf-driven pulse below takes over the sun's and
            // vignette's stop colors and oscillates them
            // indefinitely.
            act_2_start => {
                welcome_color: TweenTo::new(
                    srgb_tuple(COLOR_HEADLINE_LIGHT),
                    Duration::from_millis(DARK_FADE_MS),
                ).ease_in_out(),
                dark_opacity: TweenTo::new(1.0, Duration::from_millis(DARK_FADE_MS)).ease_in_out(),
                vignette_opacity: TweenTo::new(1.0, Duration::from_millis(DARK_FADE_MS)).ease_in_out(),
            },
            // The glare lags the dark by a beat so it reads as
            // arriving INTO the dark scene, not painted with it.
            act_2_start + 200 => {
                glare_opacity: TweenTo::new(1.0, Duration::from_millis(1700)).ease_out(),
                glare_scale: SpringTo::new(1.0).stiffness(55.0).damping(18.0),
            },
            // ── Act 3 — welcome shuffles UP, subtitle materializes.
            act_3_start => {
                welcome_y: SpringTo::new(WELCOME_SHUFFLE_Y).stiffness(110.0).damping(20.0),
                subtitle_opacity: TweenTo::new(1.0, Duration::from_millis(800)).ease_out(),
                subtitle_y: SpringTo::new(0.0).stiffness(140.0).damping(20.0),
            },
        };
        on_cleanup(move || drop(tasks));

        // ---- Unified pulse driver --------------------------------------
        //
        // ONE `raf_loop`, ONE epoch — drives the sun's core/corona
        // colors, the vignette's corner color, AND the sun's scale
        // along a single shared sine wave. Sharing the phase is the
        // whole point: the sun "growing larger" and the vignette
        // brightest must peak at the same moment or the scene reads
        // as two unrelated animations laid over each other.
        //
        // Timing:
        // - Pulse starts at `act_2_start + 200` (same tick the
        //   glare-bloom tween begins), so its first frame writes
        //   `t=0` colors / scale=1.0 — no discontinuity from the
        //   pre-pulse static seed.
        // - The sun's entrance SpringTo on `glare_scale` is set at
        //   `act_2_start + 200` and lands near 1.0 in ~1.6 s. The
        //   pulse's `AV.set()` would normally cancel the spring; we
        //   gate scale writes until the spring has had time to
        //   settle, then start writing — `sin(0) = 0` at the gate
        //   means the takeover joins at scale 1.0 with no jump.
        // - Because the phase is the SAME formula for color and
        //   scale, gating the scale just hides the first half-period
        //   of motion; once it joins, it's already in phase with the
        //   color pulse for the rest of the page lifetime.
        let pulse_start_ms = act_2_start + 200;
        let core_av = sun_core_color.clone();
        let corona_av = sun_corona_color.clone();
        let vignette_av = vignette_corner_color.clone();
        let scale_av = glare_scale.clone();
        let pulse_task = framework_core::after_ms(pulse_start_ms, move || {
            let period_ms = SUN_PULSE_PERIOD_MS;
            // Scale takeover gate: the entrance spring needs ~1.6 s
            // to settle and we don't want `AV::set` cancelling it
            // mid-flight. We ALSO need the takeover to land on a
            // sine zero so scale writes start at 1.0 (the spring's
            // resting value) with no visible jump. The smallest
            // sin-zero moment after settle is `period / 2`; that
            // gives a slightly delayed scale start but a perfectly
            // clean handoff. After that, scale and color share a
            // sin and peak together every cycle.
            let scale_gate_ms = period_ms / 2.0;
            let epoch = framework_core::time::now_micros();
            let raf = framework_core::raf_loop(move || {
                let now = framework_core::time::now_micros();
                let elapsed_ms = (now.saturating_sub(epoch) as f64) / 1000.0;
                let phase = (elapsed_ms / period_ms) * std::f64::consts::TAU;
                let sin = phase.sin();
                // Colors: `t = (sin + 1) / 2` so dim↔bright maps to
                // the sine excursion 0..1.
                let t = ((sin + 1.0) * 0.5) as f32;
                core_av.set(lerp_color(SUN_CORE_DIM, SUN_CORE_BRIGHT, t));
                corona_av.set(lerp_color(SUN_CORONA_DIM, SUN_CORONA_BRIGHT, t));
                vignette_av.set(lerp_color(
                    VIGNETTE_CORNER_DIM,
                    VIGNETTE_CORNER_BRIGHT,
                    t,
                ));
                // Scale joins after the entrance spring settles,
                // using the SAME `sin` value as the color writes
                // so brightest-warm and largest-disc land on the
                // same frame.
                if elapsed_ms >= scale_gate_ms {
                    let scale = 1.0_f32 + SUN_PULSE_AMPLITUDE * sin as f32;
                    scale_av.set(scale);
                }
            });
            // Leak the raf handle so it lives for the page; the
            // pulse should never stop while the welcome is on screen.
            std::mem::forget(raf);
        });
        std::mem::forget(pulse_task);
    });

    // ---- Build the tree ------------------------------------------------
    //
    // Five layers in document order = z-order:
    //   1. page (light bg, relative)
    //   2. dark layer (absolute, fills page)
    //   3. glare anchor (absolute, top-right)
    //   4. content layer (absolute, fills page, flex-centered)
    //
    // The content layer holds BOTH the welcome phrase and the
    // Act 3 content — they're both present in the DOM from
    // mount, but each starts with opacity 0 via its initial AV
    // value. Once the schedule fires animations, the AV
    // subscribers paint per-frame values.

    let page = page_sheet();
    let dark_layer = dark_layer_sheet();
    let vignette = vignette_sheet();
    let vignette_top = vignette_band_sheet(VignetteEdge::Top);
    let vignette_bottom = vignette_band_sheet(VignetteEdge::Bottom);
    let vignette_left = vignette_band_sheet(VignetteEdge::Left);
    let vignette_right = vignette_band_sheet(VignetteEdge::Right);
    let glare_anchor = glare_anchor_sheet();
    let content_layer = content_layer_sheet();
    let welcome_wrap = welcome_wrapper_sheet();
    let subtitle_wrap = subtitle_wrapper_sheet();
    let headline = headline_sheet();
    let subtitle = subtitle_sheet();

    ui! {
        View(style = page) {
            // Dark wash. Opacity driven by `dark_opacity` AV.
            View(style = dark_layer) {}.bind(dark_ref)

            // Vignette — warm yellow glow around the frame edges
            // (transparent center). The wrapper carries the opacity
            // animation; four child bands (one per edge) produce
            // the rounded-rectangle silhouette via overlapping
            // linear gradients. Each band's outer stop alpha is
            // pulsed by the raf-driver in `app()`.
            View(style = vignette) {
                View(style = vignette_top) {}.bind(vignette_top_ref)
                View(style = vignette_bottom) {}.bind(vignette_bottom_ref)
                View(style = vignette_left) {}.bind(vignette_left_ref)
                View(style = vignette_right) {}.bind(vignette_right_ref)
            }.bind(vignette_ref)

            // Sun glare — single view with a radial-gradient
            // background. Core + corona stop colors are pulsed
            // (sine wave) by the raf-driver, so the sun breathes
            // in sync with the vignette glow.
            View(style = glare_anchor) {}.bind(glare_ref)

            // Content layer — holds the welcome phrase + subtitle
            // in a vertical column. Welcome stays mounted the
            // whole time; its color animates with the background
            // swap, its translate_y shuffles up in Act 3 to make
            // room for the subtitle that appears below.
            View(style = content_layer) {
                // Welcome phrase. Opacity / scale / y / color all
                // animated.
                View(style = welcome_wrap) {
                    Text(style = headline) { "Welcome to Idealyst" }.bind(welcome_text_ref)
                }.bind(welcome_ref)

                // Subtitle. Hidden at start (opacity 0 via
                // stylesheet); fades in + slides up in Act 3.
                View(style = subtitle_wrap) {
                    Text(style = subtitle) { "Your app starts here." }
                }.bind(subtitle_ref)
            }
        }
    }
}

// =============================================================================
// AnimatedValue → DOM bridge
// =============================================================================

/// Subscribe `av` so every per-frame value writes to `view_ref`'s
/// node under `prop`. Until the ref is filled (the walker hasn't
/// mounted the view yet), the listener silently skips. After mount,
/// each frame writes one inline CSS property.
///
/// The returned `Subscription` is intentionally leaked — this is
/// the page's animation, not a per-component effect, so its
/// lifetime is the page lifetime.
fn drive_av(av: &AnimatedValue<f32>, view_ref: Ref<ViewHandle>, prop: AnimProp) {
    // Leak a strong ref to the AV so its `Inner` (and the animator
    // running inside it) outlive the timeline `after_ms` closures
    // that call `av.animate(...)`. The framework's animation system
    // holds only `Weak<Inner>` from the tick driver — if every
    // strong ref drops mid-tween (which happens when the only
    // outside handles are FnOnce closures that consume themselves
    // on fire), the animation unregisters and the AV freezes at
    // whatever value the closure wrote. The welcome page is a
    // one-shot intro, so a permanent leak is fine.
    std::mem::forget(av.clone());
    let sub = av.subscribe_and_apply(move |v, _vel| {
        let value = *v;
        view_ref.with(|handle| {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                    crate::web::set_animated_f32(node, prop, value);
                }
            }
            #[cfg(target_os = "ios")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_ios::IosNode>()
                {
                    backend_ios::set_animated_f32(node, prop, value);
                }
            }
            #[cfg(target_os = "android")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_android::AndroidNode>()
                {
                    backend_android::set_animated_f32(node, prop, value);
                }
            }
            #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
            {
                let _ = (handle, value, prop);
            }
        });
    });
    std::mem::forget(sub);
}

/// Color-family counterpart of [`drive_av`], targeted at a Text
/// element. Subscribes a 4-tuple AnimatedValue (sRGB
/// `(r, g, b, a)` in `0..=1`) to a `Ref<TextHandle>` and writes
/// the channels through `set_animated_color` each frame.
///
/// On iOS this lands on the underlying `UILabel`'s `textColor`
/// (per the backend's per-widget routing in
/// `set_animated_color`), which is what makes the headline's
/// dark→light color transition visible through Act 2's wash. On
/// web the inline `color` write on the text element produces the
/// same visual effect.
fn drive_color_text_av(
    av: &AnimatedValue<(f32, f32, f32, f32)>,
    text_ref: Ref<TextHandle>,
    prop: AnimProp,
) {
    // See `drive_av` — same leak so the color AV's `Inner` survives a
    // running tween once the timeline closure that kicked it off has
    // consumed itself.
    std::mem::forget(av.clone());
    let sub = av.subscribe_and_apply(move |v, _vel| {
        let (r, g, b, a) = *v;
        text_ref.with(|handle| {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                    crate::web::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(target_os = "ios")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_ios::IosNode>()
                {
                    backend_ios::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(target_os = "android")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_android::AndroidNode>()
                {
                    backend_android::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
            {
                let _ = (handle, r, g, b, a, prop);
            }
        });
    });
    std::mem::forget(sub);
}

/// Per-stop counterpart of [`drive_color_text_av`]. Animates one
/// stop in the node's `background_gradient` via
/// `AnimProp::GradientStopColor(stop_idx)`. The view's other
/// gradient state (kind, offsets, other stops) survives — each
/// backend's per-frame writer mutates only the targeted stop.
fn drive_gradient_stop_av(
    av: &AnimatedValue<(f32, f32, f32, f32)>,
    view_ref: Ref<ViewHandle>,
    stop_idx: u8,
) {
    std::mem::forget(av.clone());
    let prop = AnimProp::GradientStopColor(stop_idx);
    let sub = av.subscribe_and_apply(move |v, _vel| {
        let (r, g, b, a) = *v;
        view_ref.with(|handle| {
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                    crate::web::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(target_os = "ios")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_ios::IosNode>()
                {
                    backend_ios::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(target_os = "android")]
            {
                if let Some(node) =
                    handle.as_any().downcast_ref::<backend_android::AndroidNode>()
                {
                    backend_android::set_animated_color(node, prop, [r, g, b, a]);
                }
            }
            #[cfg(not(any(target_arch = "wasm32", target_os = "ios", target_os = "android")))]
            {
                let _ = (handle, r, g, b, a, prop);
            }
        });
    });
    std::mem::forget(sub);
}

/// Convert four `0..=255` channel values (CSS-style) into the
/// `(r, g, b, a)` tuple the color AVs consume. Alpha is in
/// `0..=1` — match the framework's gradient stop convention.
fn srgba_tuple(r: f32, g: f32, b: f32, a: f32) -> (f32, f32, f32, f32) {
    (r / 255.0, g / 255.0, b / 255.0, a)
}

/// Linear interpolate two `(r, g, b, a)` colors at `t` in `0..=1`.
/// Used by the welcome's raf-driven pulse to compute the per-frame
/// sun + vignette colors from a sine-driven `t`.
fn lerp_color(
    a: (f32, f32, f32, f32),
    b: (f32, f32, f32, f32),
    t: f32,
) -> (f32, f32, f32, f32) {
    (
        a.0 + (b.0 - a.0) * t,
        a.1 + (b.1 - a.1) * t,
        a.2 + (b.2 - a.2) * t,
        a.3 + (b.3 - a.3) * t,
    )
}

/// Convert a `#rrggbb` (or `#rgb`) hex color to the
/// `(r, g, b, a)` tuple the color AVs use. Channels in `0..=1`,
/// alpha always 1.0 (the welcome phrase doesn't fade alpha — it
/// fades the bulk through the wrapper's opacity, which is a
/// separate property).
fn srgb_tuple(hex: &str) -> (f32, f32, f32, f32) {
    let h = hex.trim_start_matches('#');
    let parse = |s: &str| u8::from_str_radix(s, 16).unwrap_or(0) as f32 / 255.0;
    let (r, g, b) = if h.len() == 6 {
        (parse(&h[0..2]), parse(&h[2..4]), parse(&h[4..6]))
    } else if h.len() == 3 {
        let ch = |c: char| u8::from_str_radix(&c.to_string().repeat(2), 16).unwrap_or(0) as f32
            / 255.0;
        let bytes: Vec<char> = h.chars().collect();
        (ch(bytes[0]), ch(bytes[1]), ch(bytes[2]))
    } else {
        (0.0, 0.0, 0.0)
    };
    (r, g, b, 1.0)
}

// =============================================================================
// Stylesheets
// =============================================================================

/// Root frame. Light background, viewport-filling, relative
/// positioning so absolute children can pin to its edges.
fn page_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Relative),
        width: Some(pct(100.0)),
        height: Some(pct(100.0)),
        background: Some(col(COLOR_LIGHT_BG)),
        // Clip children that extend past the viewport — the
        // sun-glare anchor is offset negatively so it pokes past
        // the top-right corner, and we want the page edge (not the
        // anchor's bounding box) to be the visible boundary.
        overflow: Some(framework_core::Overflow::Hidden),
        ..Default::default()
    })
}

/// Full-page dark wash. Fills the entire viewport with the dark
/// color; its opacity is animated 0 → 1 in Act 2.
fn dark_layer_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        background: Some(col(COLOR_DARK_BG)),
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}

/// Full-page vignette overlay. A radial gradient with a
/// fully-transparent center and warm-yellow corners produces an
/// ambient "the sun is lighting the room" feel — the corners
/// glow, the center stays clear so it doesn't wash out the
/// welcome text. Stop 2 (the corner) is pulsed by the raf-driver
/// in `app()`, so the glow breathes.
/// Vignette wrapper — full-page transparent container. Just carries
/// the opacity animation; the actual warm glow comes from the four
/// child band views (see [`vignette_band_sheet`]).
fn vignette_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}

/// Which screen edge a vignette band hugs. The band's gradient
/// runs perpendicular to the edge, fading from "warm at the edge"
/// inward to "fully transparent."
#[derive(Clone, Copy)]
enum VignetteEdge {
    Top,
    Bottom,
    Left,
    Right,
}

/// Cross-axis depth of each vignette band as a fraction of the
/// containing viewport. The band is fully transparent at its
/// inner edge and ramps to the warm color at the screen edge.
/// Smaller values keep the glow hugging the very perimeter; the
/// dark interior stays clean.
const VIGNETTE_BAND_PCT: f32 = 28.0;

/// One edge band of the rounded-box vignette. Pinned to one
/// screen edge with a `VIGNETTE_BAND_PCT` cross-axis depth. Linear
/// gradient runs from transparent (inner edge) to warm (screen
/// edge); the warm stop's alpha is pulsed by the raf-driver.
fn vignette_band_sheet(edge: VignetteEdge) -> Rc<StyleSheet> {
    use framework_core::{Gradient, GradientKind, GradientStop};
    // Angle convention: `0deg` = bottom→top, so stop at offset 0
    // sits at the BOTTOM of the gradient axis and stop at offset 1
    // at the TOP. For each band we want the warm (stop 1) end at
    // the screen edge:
    // - Top band: warm at the top of its own box → angle 0deg.
    // - Bottom band: warm at the bottom → angle 180deg.
    // - Left band: warm at the left → angle 270deg.
    // - Right band: warm at the right → angle 90deg.
    let (top, bottom, left, right, width, height, angle_deg) = match edge {
        VignetteEdge::Top => (
            Some(px(0.0)),
            None,
            Some(px(0.0)),
            Some(px(0.0)),
            None,
            Some(pct(VIGNETTE_BAND_PCT)),
            0.0_f32,
        ),
        VignetteEdge::Bottom => (
            None,
            Some(px(0.0)),
            Some(px(0.0)),
            Some(px(0.0)),
            None,
            Some(pct(VIGNETTE_BAND_PCT)),
            180.0,
        ),
        VignetteEdge::Left => (
            Some(px(0.0)),
            Some(px(0.0)),
            Some(px(0.0)),
            None,
            Some(pct(VIGNETTE_BAND_PCT)),
            None,
            270.0,
        ),
        VignetteEdge::Right => (
            Some(px(0.0)),
            Some(px(0.0)),
            None,
            Some(px(0.0)),
            Some(pct(VIGNETTE_BAND_PCT)),
            None,
            90.0,
        ),
    };
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top,
        bottom,
        left,
        right,
        width,
        height,
        background_gradient: Some(Gradient {
            kind: GradientKind::Linear { angle_deg },
            // `[transparent, warm]` — the pulse driver writes
            // index 1 (the "warm" stop) every frame; index 0
            // stays fully transparent so the band fades out into
            // the page interior smoothly.
            stops: vec![
                GradientStop { offset: 0.0, color: Color("rgba(255, 168, 60, 0.0)".into()) },
                GradientStop { offset: 1.0, color: Color("rgba(255, 168, 60, 0.0)".into()) },
            ],
        }),
        ..Default::default()
    })
}

/// Anchor box for the sun glare. Positioned fully on-screen in the
/// top-right corner — children that extend past the host view's
/// bounds aren't rendered on iOS (UIKit clips them despite
/// `clipsToBounds = NO` somewhere up the chain), so the
/// off-screen-bleed feel is produced by shadow blur on the glare
/// layers instead.
fn glare_anchor_sheet() -> Rc<StyleSheet> {
    use framework_core::{Gradient, GradientKind, GradientStop, RadialExtent};
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        // Pin the anchor's bounding box to the viewport's top-right
        // edge, then shift it by half its own width on each axis so
        // its CENTER lands on the corner. The visible quadrant is
        // the lower-left of the disc; the scale pulse still emits
        // from the disc's geometric center (which is the corner
        // itself) since transform-origin is the default 50% 50%.
        top: Some(px(0.0)),
        right: Some(px(0.0)),
        margin_top: Some(pct(GLARE_CENTER_SHIFT_PCT)),
        margin_right: Some(pct(GLARE_CENTER_SHIFT_PCT)),
        // Sun is sized as a fraction of viewport width and held
        // square via `aspect_ratio: 1.0`. The layout engine sets
        // height to match the resolved width, so the disc remains
        // circular on phones AND tablets without per-device tuning.
        width: Some(pct(GLARE_ANCHOR_SIZE_PCT)),
        aspect_ratio: Some(1.0),
        // Clip to a perfect circle. CSS-style "max radius" — each
        // backend clamps to half the smaller side (iOS's
        // `apply_style_to_view` handles this explicitly because
        // UIKit's `cornerRadius` doesn't clamp on its own).
        border_top_left_radius: Some(px(999.0)),
        border_top_right_radius: Some(px(999.0)),
        border_bottom_left_radius: Some(px(999.0)),
        border_bottom_right_radius: Some(px(999.0)),
        // `Overflow::Hidden` ensures the gradient sublayer is clipped
        // to the rounded corner. iOS's cornerRadius path already sets
        // `clipsToBounds=true`, but stating it explicitly here makes
        // the circle behavior an author intent visible at the call
        // site (and protects against future backend changes that
        // might decouple radius from clipping).
        overflow: Some(framework_core::Overflow::Hidden),
        opacity: Some(Tokenized::Literal(0.0)),
        // Radial gradient: bright cream core → warm gold corona →
        // soft orange halo → transparent edge. The transparent
        // outermost stop produces the soft falloff that used to
        // require stacked partial-alpha discs.
        background_gradient: Some(Gradient {
            kind: GradientKind::Radial {
                center: (0.5, 0.5),
                radius: 1.0,
                // The sun's anchor is aspect-ratio:1 (a square),
                // so ClosestSide puts the transparent edge stop
                // exactly at the view boundary — the gradient
                // fills the whole circular clip.
                extent: RadialExtent::ClosestSide,
            },
            // Four-stop falloff tuned for the larger anchor: bright
            // cream core kept tight (offset 0–0.18) so the hot
            // center reads as a sun, then a long, mostly-transparent
            // tail that fades out gently to the edge of the disc.
            // Each ring's alpha is roughly half the previous, which
            // gives a perceptually-even brightness ramp (alpha is
            // gamma-space additive, the eye expects exponential
            // falloff for a "smooth" gradient).
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: Color(COLOR_SUN_CORE.into()),
                },
                GradientStop {
                    offset: 0.18,
                    color: Color("rgba(255, 210, 110, 0.70)".into()),
                },
                GradientStop {
                    offset: 0.45,
                    color: Color("rgba(255, 168, 60, 0.22)".into()),
                },
                GradientStop {
                    offset: 0.75,
                    color: Color("rgba(255, 168, 60, 0.06)".into()),
                },
                GradientStop {
                    offset: 1.0,
                    color: Color("rgba(255, 168, 60, 0.0)".into()),
                },
            ],
        }),
        ..Default::default()
    })
}

/// Absolutely-positioned column that holds the welcome phrase and
/// the subtitle. Flex-centered so the pair sits in the middle of
/// the page; the welcome ends up slightly above center, subtitle
/// slightly below.
fn content_layer_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        // Modest gap between the headline and where the subtitle
        // lands. The welcome's Act 3 shuffle-up adds visual breathing
        // room on top of this.
        gap: Some(px(14.0)),
        ..Default::default()
    })
}

/// Wrapper around the welcome phrase. Opacity 0 at start (the
/// `welcome_opacity` AV animates it up in Act 1).
fn welcome_wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        opacity: Some(Tokenized::Literal(0.0)),
        // Dark ink at rest; `welcome_color` AV transitions this to
        // the light cream during Act 2. The animated inline color
        // overrides this initial value.
        color: Some(col(COLOR_HEADLINE_DARK)),
        ..Default::default()
    })
}

/// Wrapper around the subtitle. Opacity 0 at start — Act 3 fades
/// it in and slides it up.
fn subtitle_wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Center),
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}

/// Headline text style — the welcome phrase wears this. The
/// initial `color` value matches `COLOR_HEADLINE_DARK` so the
/// first paint reads correctly on the light frame; once the
/// timeline kicks in, the AV-driven inline color override on the
/// UILabel (iOS) / `style.color` (web) carries the dark→light
/// transition through Act 2.
fn headline_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(HEADLINE_SIZE_PX)),
        font_weight: Some(FontWeight::Bold),
        letter_spacing: Some(Tokenized::Literal(-1.6)),
        line_height: Some(Tokenized::Literal(HEADLINE_SIZE_PX + 8.0)),
        text_align: Some(TextAlign::Center),
        color: Some(col(COLOR_HEADLINE_DARK)),
        ..Default::default()
    })
}

/// Subtitle under the Act 3 headline.
fn subtitle_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(SUBTITLE_SIZE_PX)),
        font_weight: Some(FontWeight::Normal),
        letter_spacing: Some(Tokenized::Literal(0.6)),
        line_height: Some(Tokenized::Literal(SUBTITLE_SIZE_PX + 8.0)),
        text_align: Some(TextAlign::Center),
        color: Some(col(COLOR_SUBTITLE_LIGHT)),
        ..Default::default()
    })
}

fn static_sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

fn pct(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Percent(v))
}

fn col(s: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(s.into()))
}
