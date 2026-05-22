//! The `app()` function — the entry point invoked via
//! `framework_core::mount(backend, super::app)` from each platform
//! wrapper. Owns the timeline, AVs, refs, and the unified raf-driven
//! pulse that breathes the sun + vignette + planets for the page
//! lifetime.

use std::time::Duration;

use framework_core::animation::{AnimProp, AnimatedValue, SpringTo, TweenTo};
use framework_core::{
    animated, effect, node_ref, on_cleanup, timeline, ui, Primitive, Ref, TextHandle, ViewHandle,
};

use crate::animation_bridge::{drive_av, drive_color_text_av, drive_gradient_stop_av};
use crate::color::{lerp_color, srgb_tuple, srgba_tuple};
use crate::components::content_layer::content_layer_sheet;
use crate::components::dark_layer::dark_layer_sheet;
use crate::components::page::page_sheet;
use crate::components::planet::{
    planet_sheet, PLANETS, PLANET_FADE_IN_MS, PLANET_SCALE_BACK, PLANET_SCALE_FRONT,
};
use crate::components::subtitle::{subtitle_sheet, subtitle_wrapper_sheet, SUBTITLE_ENTER_Y};
use crate::components::sun_glare::{
    glare_anchor_sheet, glare_wrapper_sheet, COLOR_SUN_CORE, GLARE_INITIAL_SCALE,
    SUN_CORE_BRIGHT, SUN_CORE_DIM, SUN_CORONA_BRIGHT, SUN_CORONA_DIM, SUN_PULSE_AMPLITUDE,
    SUN_PULSE_PERIOD_MS,
};
use crate::components::vignette::{
    vignette_band_sheet, vignette_sheet, VignetteEdge, VIGNETTE_CORNER_BRIGHT,
    VIGNETTE_CORNER_DIM,
};
use crate::components::welcome_phrase::{
    headline_sheet, welcome_wrapper_sheet, COLOR_HEADLINE_DARK, COLOR_HEADLINE_LIGHT,
    PHRASE_ENTER_SCALE, PHRASE_ENTER_Y, WELCOME_SHUFFLE_Y,
};
use crate::constants::{
    ACT_1_HOLD_MS, CONTENT_OFFSET_AFTER_GLARE_MS, DARK_FADE_MS, INTRO_PAUSE_MS,
    PHRASE_ENTER_BUDGET_MS,
};

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

    // Planet animation values — one quad (x, y, back-alpha,
    // front-alpha) per planet. Position AVs are SHARED between
    // back and front views (they're at the same place); only the
    // opacity AVs differ so the z-swap reads correctly. See
    // `PLANETS` for orbit config.
    let planet_x: [AnimatedValue<f32>; 3] = [
        animated!(0.0_f32),
        animated!(0.0_f32),
        animated!(0.0_f32),
    ];
    let planet_y: [AnimatedValue<f32>; 3] = [
        animated!(0.0_f32),
        animated!(0.0_f32),
        animated!(0.0_f32),
    ];
    let planet_back_alpha: [AnimatedValue<f32>; 3] = [
        animated!(0.0_f32),
        animated!(0.0_f32),
        animated!(0.0_f32),
    ];
    let planet_front_alpha: [AnimatedValue<f32>; 3] = [
        animated!(0.0_f32),
        animated!(0.0_f32),
        animated!(0.0_f32),
    ];
    // Scale per planet — driven by the orbit depth (sin θ). At
    // depth = -1 the planet is at the back of its orbit, drawn
    // small (PLANET_SCALE_BACK); at depth = +1 it's at the
    // front, drawn large (PLANET_SCALE_FRONT). Linear interp.
    let planet_scale: [AnimatedValue<f32>; 3] = [
        animated!(1.0_f32),
        animated!(1.0_f32),
        animated!(1.0_f32),
    ];

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

    // Page ref — read at raf-time to get viewport dimensions for
    // computing orbit positions in absolute pixels. The orbit
    // semi-axes are viewport-fractional, so the orbit scales with
    // window size on web and follows the safe-area on mobile.
    let page_ref = node_ref!(ViewHandle);

    // Two refs per planet — one for the view BEFORE the content
    // layer in document order (z-back) and one AFTER (z-front).
    // Both share `planet_x` / `planet_y` so they always overlap;
    // opposite-phase opacity AVs make exactly one visible at a
    // time as the orbit crosses the horizontal axis.
    let planet_back_refs: [Ref<ViewHandle>; 3] = [
        node_ref!(ViewHandle),
        node_ref!(ViewHandle),
        node_ref!(ViewHandle),
    ];
    let planet_front_refs: [Ref<ViewHandle>; 3] = [
        node_ref!(ViewHandle),
        node_ref!(ViewHandle),
        node_ref!(ViewHandle),
    ];

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

    // Planet wiring: each planet has TWO views (back + front)
    // sharing position AVs but with opposite-phase opacity AVs.
    // The for-loop is identical for both halves, just keyed by
    // the alpha AV that's specific to that half.
    for i in 0..3 {
        drive_av(&planet_x[i], planet_back_refs[i], AnimProp::TranslateX);
        drive_av(&planet_y[i], planet_back_refs[i], AnimProp::TranslateY);
        drive_av(&planet_back_alpha[i], planet_back_refs[i], AnimProp::Opacity);
        drive_av(&planet_scale[i], planet_back_refs[i], AnimProp::Scale);
        drive_av(&planet_x[i], planet_front_refs[i], AnimProp::TranslateX);
        drive_av(&planet_y[i], planet_front_refs[i], AnimProp::TranslateY);
        drive_av(&planet_front_alpha[i], planet_front_refs[i], AnimProp::Opacity);
        drive_av(&planet_scale[i], planet_front_refs[i], AnimProp::Scale);
    }

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
        // Clone planet AVs + page ref into the closure. The closure
        // gets moved into `raf_loop`, which holds it for the page
        // lifetime; the AV clones share Rc<Inner> with the originals
        // so writes propagate to bound views the same way.
        let planet_x_clones: [AnimatedValue<f32>; 3] = [
            planet_x[0].clone(),
            planet_x[1].clone(),
            planet_x[2].clone(),
        ];
        let planet_y_clones: [AnimatedValue<f32>; 3] = [
            planet_y[0].clone(),
            planet_y[1].clone(),
            planet_y[2].clone(),
        ];
        let planet_back_alpha_clones: [AnimatedValue<f32>; 3] = [
            planet_back_alpha[0].clone(),
            planet_back_alpha[1].clone(),
            planet_back_alpha[2].clone(),
        ];
        let planet_front_alpha_clones: [AnimatedValue<f32>; 3] = [
            planet_front_alpha[0].clone(),
            planet_front_alpha[1].clone(),
            planet_front_alpha[2].clone(),
        ];
        let planet_scale_clones: [AnimatedValue<f32>; 3] = [
            planet_scale[0].clone(),
            planet_scale[1].clone(),
            planet_scale[2].clone(),
        ];
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

                // ---- Planets ---------------------------------------
                //
                // Orbit center = top-right corner of viewport (sun
                // position). Read live viewport dims via `page_ref`
                // when available so the orbit scales with resize /
                // rotation; otherwise fall back to a sensible
                // portrait-phone default (393×800 dp) so the system
                // still animates even if the handle isn't queryable.
                let viewport = page_ref.with(|h| h.frame()).flatten();
                let (vw, vh) = viewport
                    .map(|r| (r.width, r.height))
                    .filter(|(w, h)| *w > 0.0 && *h > 0.0)
                    .unwrap_or((393.0, 800.0));
                let fade_in =
                    ((elapsed_ms / PLANET_FADE_IN_MS).min(1.0)) as f32;
                // Orbit: diagonal ellipse centred on the SCREEN
                // CENTRE, with its major axis at 45° (upper-right
                // ↔ lower-left). Sized so the major-axis extremes
                // reach beyond the vertical bounds of the welcome
                // text. Reads as a 3D circular orbit viewed from
                // above the orbit plane — the planet appears LARGE
                // on the front arc (sin θ > 0) and SMALL on the
                // back arc (sin θ < 0), with the size change
                // selling the depth cue.
                let cx = vw * 0.50;
                let cy = vh * 0.50;
                // Major axis direction in screen — 45° diagonal,
                // pointing from upper-right toward lower-left.
                // `1/√2` is the projection of a unit vector on
                // each axis at exactly 45°.
                let major_x: f32 = -std::f32::consts::FRAC_1_SQRT_2;
                let major_y: f32 = std::f32::consts::FRAC_1_SQRT_2;
                // Minor axis perpendicular (rotate major 90° CW
                // in screen coords: (x, y) → (y, -x)) — points
                // from upper-left toward lower-right.
                let minor_x: f32 = major_y;
                let minor_y: f32 = -major_x;
                for (i, cfg) in PLANETS.iter().enumerate() {
                    let theta = (elapsed_ms / cfg.period_ms)
                        * std::f64::consts::TAU
                        + cfg.phase_offset as f64;
                    let cos_t = theta.cos() as f32;
                    let sin_t = theta.sin() as f32;
                    // Both `r_major` and `r_minor` scale with
                    // viewport HEIGHT so the orbit's vertical
                    // reach is what the spec calls for (must
                    // extend past the text top/bottom). The
                    // diagonal direction means a unit of
                    // `r_major` contributes `1/√2` to both x and
                    // y — so vertical reach = `r_major / √2`.
                    let r_major = vh * cfg.ry_frac;
                    let r_minor = vh * cfg.rx_frac;
                    let offset_x = r_major * cos_t * major_x
                        + r_minor * sin_t * minor_x;
                    let offset_y = r_major * cos_t * major_y
                        + r_minor * sin_t * minor_y;
                    let center_x = cx + offset_x;
                    let center_y = cy + offset_y;
                    // Planet view sits at `top:0, right:0` with
                    // size = `size_dp`, so its natural top-left is
                    // at (vw - size_dp, 0). Convert the desired
                    // centre into a translate from that position.
                    let tx = center_x - vw + cfg.size_dp * 0.5;
                    let ty = center_y - cfg.size_dp * 0.5;
                    planet_x_clones[i].set(tx);
                    planet_y_clones[i].set(ty);
                    // Depth = sin θ. Lerp scale from BACK (depth=-1)
                    // to FRONT (depth=+1). Lerp parameter `t` maps
                    // -1..+1 → 0..1.
                    let depth_t = (sin_t + 1.0) * 0.5;
                    let scale = PLANET_SCALE_BACK
                        + (PLANET_SCALE_FRONT - PLANET_SCALE_BACK)
                            * depth_t;
                    planet_scale_clones[i].set(scale);
                    // Z-swap with a soft crossfade band around the
                    // side passes (where depth crosses zero).
                    // Outside the band: ONE view is fully visible,
                    // the other fully hidden. Inside the band:
                    // both share the alpha, summing to ~1.0 so the
                    // planet never fades to nothing — only the
                    // z-order swaps. Without this the planet would
                    // disappear twice per orbit when sin θ ≈ 0.
                    const CROSSFADE_HALF_WIDTH: f32 = 0.10;
                    let front_t = ((sin_t + CROSSFADE_HALF_WIDTH)
                        / (2.0 * CROSSFADE_HALF_WIDTH))
                        .clamp(0.0, 1.0);
                    let back_t = 1.0 - front_t;
                    planet_front_alpha_clones[i].set(front_t * fade_in);
                    planet_back_alpha_clones[i].set(back_t * fade_in);

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
    let glare_wrapper = glare_wrapper_sheet();
    let glare_anchor = glare_anchor_sheet();
    let content_layer = content_layer_sheet();
    let welcome_wrap = welcome_wrapper_sheet();
    let subtitle_wrap = subtitle_wrapper_sheet();
    let headline = headline_sheet();
    let subtitle = subtitle_sheet();

    // Planet sheets — one per planet, sized by the config. Two
    // copies of each are mounted (back + front of content) and
    // share the same StyleSheet content, so the framework dedups
    // them into one CSS class on web.
    let planet_sheet_0 = planet_sheet(PLANETS[0].size_dp, PLANETS[0].color);
    let planet_sheet_1 = planet_sheet(PLANETS[1].size_dp, PLANETS[1].color);
    let planet_sheet_2 = planet_sheet(PLANETS[2].size_dp, PLANETS[2].color);
    let planet_back_0 = planet_back_refs[0];
    let planet_back_1 = planet_back_refs[1];
    let planet_back_2 = planet_back_refs[2];
    let planet_front_0 = planet_front_refs[0];
    let planet_front_1 = planet_front_refs[1];
    let planet_front_2 = planet_front_refs[2];

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

            // Sun glare — wrapper does the responsive corner-
            // centering via `translate(50%, -50%)`; inner disc
            // holds the gradient + opacity + scale + per-stop
            // color animations. Separating the two keeps the
            // animated transform from clobbering the static
            // centering translate on iOS.
            View(style = glare_wrapper) {
                View(style = glare_anchor) {}.bind(glare_ref)
            }

            // Planets (back half) — these render BEFORE the content
            // layer in document order, which puts them behind the
            // welcome text. The raf-driver gates their opacity via
            // a smooth crossfade against the front view's.
            View(style = planet_sheet_0.clone()) {}.bind(planet_back_0)
            View(style = planet_sheet_1.clone()) {}.bind(planet_back_1)
            View(style = planet_sheet_2.clone()) {}.bind(planet_back_2)

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

            // Planets (front half) — same set, rendered AFTER the
            // content layer so they appear in front of the welcome
            // text. Front planet opacity stays at 1.0 when the
            // crossfade favours the front.
            View(style = planet_sheet_0) {}.bind(planet_front_0)
            View(style = planet_sheet_1) {}.bind(planet_front_1)
            View(style = planet_sheet_2) {}.bind(planet_front_2)
        }.bind(page_ref)
    }
}
