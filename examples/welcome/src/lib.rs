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

/// Sun-glare anchor size as a fraction of viewport HEIGHT. The
/// stylesheet pairs this with `aspect_ratio: 1.0` so the box stays
/// square; the layout engine derives width from height. Height-
/// relative ties the sun's apparent size to the vertical extent
/// of the screen — feels more grounded than width-relative, where
/// a wide landscape window would push the sun comically large.
///
/// Note: the wrapper sits at the top-right corner with
/// `translate(50%, -50%)`, so only the bottom-left **quadrant** of
/// the disc is on-screen. Effective visible reach is therefore
/// roughly `height_pct / 2` along each axis — `60%` means the
/// bloom extends ~30% of viewport height into the page, which
/// reads as a hero light source rather than a decorative dot.
const GLARE_ANCHOR_HEIGHT_PCT: f32 = 60.0;


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

// ---- Planet system -------------------------------------------------------
//
// Three planets orbit the sun on tight elliptical paths sized to pass
// across the welcome text in the middle of the viewport. Each planet
// is rendered TWICE — once before the content layer (back), once
// after (front) — sharing position AVs but with opposite-phase
// opacity AVs. The raf-driver writes `opacity = max(sin θ, 0)` on
// the front view and `max(-sin θ, 0)` on the back view, so a single
// planet "passes behind" the text when its orbit angle is in the
// upper half of the circle (sin < 0) and "in front of" when in the
// lower half. The z-swap is the whole point of the duplication —
// the framework doesn't have a per-frame z-index animation, but
// document-order render + opacity flip-flop is equivalent.

/// Per-planet config. `rx_frac` / `ry_frac` are fractions of viewport
/// width / height for the elliptical semi-axes (orbit center is the
/// top-right corner, where the sun lives). `period_ms` is one full
/// revolution; `phase_offset` (radians) staggers the three planets
/// so they don't all line up. `size_dp` is the dot diameter;
/// `color` is a CSS string.
struct PlanetConfig {
    rx_frac: f32,
    ry_frac: f32,
    period_ms: f64,
    phase_offset: f32,
    size_dp: f32,
    color: &'static str,
}

/// Three planets at increasing radii / decreasing speeds — closer
/// to the sun = faster, like real Keplerian orbits. The middle
/// planet's orbit (`rx ≈ 0.5 * vw`, `ry ≈ 0.5 * vh`) passes
/// directly through the welcome text's bbox, so the z-swap is most
/// visible there. The inner planet is small + tight + fast; the
/// outer is larger + wider + slow, sweeping past the bottom-left
/// corner once every ~20 s.
const PLANETS: [PlanetConfig; 3] = [
    // Each planet orbits the welcome text (NOT the sun) on a
    // diagonally-tilted plane. `rx_frac` is the 2D ellipse's
    // semi-major (along the diagonal) as fraction of viewport
    // WIDTH; `ry_frac` is the semi-minor (perpendicular) as
    // fraction of viewport HEIGHT. The tilt is implicit in the
    // 2D vs depth axes — the same `sin(θ)` that swings the
    // planet along the minor axis ALSO drives its depth, so
    // every planet visit along the orbit has a depth that
    // matches the perpendicular sway. Combined with the
    // scale animation (`small back ↔ large front`), this reads
    // as a real 3D circle viewed from above the orbit plane.
    // All three orbits share the centre (viewport centre) and
    // the 45° diagonal major axis; they differ in size + speed.
    // `ry_frac` × vh sizes the diagonal MAJOR semi-axis; the
    // major direction is at 45° so the orbit's vertical reach
    // is `ry_frac × vh / √2`. To extend past the welcome text
    // (which roughly spans the middle 30% of the viewport
    // vertically), the middle planet uses `ry_frac ≈ 0.45` →
    // vertical reach ≈ 0.32 × vh, clearing the text by ~17%
    // on each side.
    // For a 45° diagonal major axis, a unit of `r_major`
    // contributes 1/√2 ≈ 0.707 to BOTH the horizontal and
    // vertical reach. So if `r_major = 0.30 × vh` on a 393×852
    // viewport, the orbit spans ±0.21 × vh ≈ ±180 px in both
    // x and y from the centre — fits horizontally (vw/2 = 197)
    // and clears the text vertically (text band ≈ 350-540, so
    // half-text = 95 px). Bigger `r_major` lets the diagonal
    // extremes hang off the left/right edges (intentional for
    // the outer planet).
    PlanetConfig {
        // Inner — small orbit, fast. Stays well inside the
        // viewport, extends just past the text vertically.
        // Vertical reach ≈ 0.14 × vh = 119 px.
        rx_frac: 0.10,  // semi-MINOR (perp to diagonal) as frac of vh
        ry_frac: 0.20,  // semi-MAJOR (along diagonal) as frac of vh
        period_ms: 8000.0,
        phase_offset: 0.0,
        size_dp: 8.0,
        color: "#c9b88c",
    },
    PlanetConfig {
        // Middle — extends comfortably past the text. Vertical
        // reach ≈ 0.21 × vh = 180 px.
        rx_frac: 0.15,
        ry_frac: 0.30,
        period_ms: 13000.0,
        phase_offset: 2.09,
        size_dp: 14.0,
        color: "#c9b88c",
    },
    PlanetConfig {
        // Outer — broad sweep, just past the horizontal limit
        // so the diagonal extremes graze the left/right edges
        // of the viewport. Vertical reach ≈ 0.24 × vh = 209 px.
        // Slowest period gives a stately feel.
        rx_frac: 0.20,
        ry_frac: 0.34,
        period_ms: 20000.0,
        phase_offset: 4.18,
        size_dp: 11.0,
        color: "#c9b88c",
    },
];

/// Scale at the orbit's back extreme (depth = -1). Sub-1.0 so
/// the planet visibly shrinks when it's "behind" the welcome
/// text. The contrast against `PLANET_SCALE_FRONT` is the main
/// depth cue in the 3D illusion.
const PLANET_SCALE_BACK: f32 = 0.45;

/// Scale at the orbit's front extreme (depth = +1). >1.0 so
/// the planet visibly grows when it's "in front of" the text.
const PLANET_SCALE_FRONT: f32 = 1.55;

/// How long the planets take to fade in from invisible once the
/// raf-driver starts running (= Act 2 + 200 ms, same as sun
/// bloom). Without a fade-in, the planet whose `phase_offset`
/// puts it on the lower half at t=0 would pop on at non-zero
/// alpha; this ramps the whole system up smoothly.
const PLANET_FADE_IN_MS: f64 = 1500.0;


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
                let raw_frame = viewport;
                let (vw, vh) = viewport
                    .map(|r| (r.width, r.height))
                    .filter(|(w, h)| *w > 0.0 && *h > 0.0)
                    .unwrap_or((393.0, 800.0));
                #[cfg(target_os = "ios")]
                {
                    // Log once every ~2s
                    if (elapsed_ms as i32) % 2000 < 17 {
                        backend_ios_core::ios_log(&format!(
                            "[planet] viewport raw={:?} vw={:.1} vh={:.1} cx={:.1} cy={:.1}",
                            raw_frame,
                            vw,
                            vh,
                            vw * 0.50,
                            vh * 0.50,
                        ));
                    }
                }
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
                    #[cfg(target_os = "ios")]
                    {
                        if i == 1 && (elapsed_ms as i32) % 1000 < 17 {
                            backend_ios_core::ios_log(&format!(
                                "[planet i={}] theta={:.2} center=({:.1},{:.1}) tx={:.1} ty={:.1}",
                                i,
                                theta,
                                center_x,
                                center_y,
                                center_x - vw + cfg.size_dp * 0.5,
                                center_y - cfg.size_dp * 0.5,
                            ));
                        }
                    }
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
            // welcome text. The raf-driver gates their opacity by
            // `max(-sin θ, 0)` so they only appear when the orbit
            // angle is in the upper half (where the planet is
            // "behind" the sun in the implicit 3D model).
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
            // text. Opacity-gated by `max(sin θ, 0)` (opposite of
            // the back set), so exactly one half of each planet
            // pair is visible at any frame.
            View(style = planet_sheet_0) {}.bind(planet_front_0)
            View(style = planet_sheet_1) {}.bind(planet_front_1)
            View(style = planet_sheet_2) {}.bind(planet_front_2)
        }.bind(page_ref)
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

/// One planet body — a solid-color circle, positioned at the
/// viewport's top-right corner by default (same anchor as the sun)
/// and moved into orbit by the raf-driver via animated translate.
/// Two of these are rendered per `PlanetConfig` — one before the
/// content layer (z-behind) and one after (z-in-front); the
/// per-frame opacity AVs gate visibility so exactly one is on
/// screen at a time. Sized in dp because translate writes from
/// the raf-driver are in dp too — keeping the unit consistent
/// across the orbit math is what makes the centering arithmetic
/// (`tx = center_x - vw + size_dp/2`) work without conversion.
fn planet_sheet(size_dp: f32, color: &'static str) -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        right: Some(px(0.0)),
        width: Some(px(size_dp)),
        height: Some(px(size_dp)),
        background: Some(col(color)),
        // Half-side radius — turns the square into a circle on
        // every backend's clamp path (iOS / web / Android all
        // clamp `border-radius > min(w, h)/2` to that value).
        border_top_left_radius: Some(px(999.0)),
        border_top_right_radius: Some(px(999.0)),
        border_bottom_left_radius: Some(px(999.0)),
        border_bottom_right_radius: Some(px(999.0)),
        // Start invisible — the raf-driver writes the real
        // opacity (one half of `sin θ`) once it begins running
        // at Act 2 + 200 ms.
        opacity: Some(Tokenized::Literal(0.0)),
        ..Default::default()
    })
}

/// Positioned wrapper for the sun. Pinned to the viewport's
/// top-right edge then translated by HALF its own dimensions on
/// each axis — `translate(50%, -50%)` is BOX-relative in CSS, so
/// the shift is always exactly half the wrapper's size on any
/// device. Half the disc hangs offscreen by design: the radial
/// gradient reads as a light source cresting through the top-right
/// corner rather than as a visible circle.
///
/// The wrapper carries ONLY the static layout / static transform.
/// The animated disc lives inside it (see [`glare_anchor_sheet`])
/// so per-frame writes to scale don't clobber this translate —
/// iOS in particular bakes scale + translate into a single
/// `CGAffineTransform`, and any animated write would otherwise
/// overwrite our centering offset.
fn glare_wrapper_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        right: Some(px(0.0)),
        // Size by HEIGHT (aspect_ratio:1.0 makes the box square,
        // deriving width from height). Reads as a screen-vertical
        // anchor — taller phone = bigger sun.
        height: Some(pct(GLARE_ANCHOR_HEIGHT_PCT)),
        aspect_ratio: Some(1.0),
        transform: Some(vec![
            framework_core::Transform::TranslateX(Length::Percent(50.0)),
            framework_core::Transform::TranslateY(Length::Percent(-50.0)),
        ]),
        ..Default::default()
    })
}

/// Inner disc: fills the wrapper, holds the gradient, takes the
/// per-frame opacity + scale + stop-color animations. Scale
/// animation pivots from the disc's own center (default
/// transform-origin), which the wrapper has placed exactly on
/// the viewport's top-right corner — so the pulse "breathes
/// from the corner" without any per-axis math here.
fn glare_anchor_sheet() -> Rc<StyleSheet> {
    use framework_core::{Gradient, GradientKind, GradientStop, RadialExtent};
    static_sheet(StyleRules {
        position: Some(Position::Absolute),
        // Fill the wrapper.
        top: Some(px(0.0)),
        right: Some(px(0.0)),
        bottom: Some(px(0.0)),
        left: Some(px(0.0)),
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
                    offset: 0.30,
                    color: Color("rgba(255, 210, 110, 0.70)".into()),
                },
                GradientStop {
                    offset: 0.55,
                    color: Color("rgba(255, 168, 60, 0.22)".into()),
                },
                GradientStop {
                    offset: 0.80,
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
