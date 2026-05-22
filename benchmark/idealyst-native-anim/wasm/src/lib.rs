//! Idealyst-native variant for the animation benchmark suite.
//!
//! Renders N circles inside a fixed 800×600 stage and animates their
//! positions per frame. The framework's idiomatic per-frame surface
//! is used:
//!
//!   - One `AnimatedValue<f32>` per ball per axis (2N AVs total).
//!   - Each AV is `.bind()`'d to its ball's `Ref<ViewHandle>` →
//!     `AnimProp::TranslateX/Y`. Framework wires the subscription
//!     so `av.set(x)` propagates to a backend-side `style.transform`
//!     write.
//!   - A single `raf_loop` advances the bounce sim and pushes new
//!     (x, y) values into the AVs. No per-AV animator is installed —
//!     we drive imperatively, matching the welcome-coordinator pattern
//!     (see [[project_third_party_extension]] for the broader
//!     pattern of framework-as-write-surface).
//!
//! See [benchmark/anim/spec.md](../../anim/spec.md) for the per-hook
//! contract this crate honors.

use std::cell::RefCell;
use std::rc::Rc;

use backend_web::WebBackend;
use framework_core::animation::{AnimProp, AnimatedValue, SpringTo};
use framework_core::{
    node_ref, render, signal, ui, Color, Length, Owner, Position, Primitive, RafLoop, Ref, Signal,
    StyleRules, StyleSheet, TokenEntry, Tokenized, ViewHandle,
};
use idea_ui::{install_theme, ThemeTokens};
use std::time::Duration;
use wasm_bindgen::prelude::*;
use web_sys::Performance;

// =============================================================================
// Physics constants — MUST match benchmark/anim/harness.js
// =============================================================================

const VIEWPORT_W: f32 = 800.0;
const VIEWPORT_H: f32 = 600.0;
const BALL_RADIUS: f32 = 4.0;
const VEL_RANGE: f32 = 200.0;
/// Fixed timestep — MUST be f64. JS's `1/60` is a Number (f64). Storing
/// as f32 here, then casting to f64 at use site, produces a *different*
/// bit pattern than JS's pure-f64 1/60 (the f32-truncated value
/// promoted back to f64 has different low bits). 60 multiplications
/// compound the difference into ~1e-3 drift, which exceeds the
/// determinism tolerance. Pin as f64 to match JS exactly.
const FIXED_DT: f64 = 1.0 / 60.0;

// =============================================================================
// Mulberry32 — bit-identical to harness.js mulberry32
// =============================================================================
//
// JS's `Math.imul` returns the low 32 bits of a signed multiply;
// `u32::wrapping_mul` matches that bit pattern. The float
// conversion (`(u >>> 8) / 2^24`) is closed-form in IEEE-754
// arithmetic and produces the same f64 in V8 and Rust at the
// boundary we test against (1e-3 tolerance leaves slack for any
// downstream FP order-of-ops drift after frame 60).

struct Mulberry32 {
    state: u32,
}

impl Mulberry32 {
    fn new(seed: u32) -> Self {
        Self { state: seed }
    }
    /// Snapshot the current state. Used by springstorm to advance the
    /// re-kick RNG across rAF ticks: each tick rebuilds the RNG from
    /// the saved state, draws N targets, then saves the advanced
    /// state back into STORE.
    fn state(&self) -> u32 {
        self.state
    }
    fn next(&mut self) -> u32 {
        self.state = self.state.wrapping_add(0x6D2B79F5);
        let mut t = self.state;
        t = (t ^ (t >> 15)).wrapping_mul(t | 1);
        t ^= t.wrapping_add((t ^ (t >> 7)).wrapping_mul(t | 61));
        t ^ (t >> 14)
    }
    /// `[0, 1)` Float64 using the high 24 bits of the next u32. Matches
    /// `(u >>> 8) / 16777216` in JS — and crucially returns f64 (not
    /// f32) so all downstream arithmetic stays in f64, the same
    /// precision V8 uses when computing from Float32Array reads.
    fn next_float(&mut self) -> f64 {
        let u = self.next();
        ((u >> 8) as f64) / 16_777_216.0
    }
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next_float()
    }
}

// Storage layout matches JS's Float32Array — when JS reads from a
// Float32Array, the value is promoted to f64; arithmetic happens in
// f64; the result is truncated back to f32 on write. We mirror that
// exactly: store in f32, compute in f64. Otherwise a tight loop of
// `pos += vel * dt` in f32-only accumulates enough float drift to
// exceed the 1e-3 determinism tolerance after ~60 frames.

fn bounce_initial(n: usize, seed: u32) -> Vec<f32> {
    let r = BALL_RADIUS as f64;
    let w = VIEWPORT_W as f64;
    let h = VIEWPORT_H as f64;
    let v = VEL_RANGE as f64;
    let mut rng = Mulberry32::new(seed);
    let mut out = vec![0.0_f32; 4 * n];
    for i in 0..n {
        out[4 * i]     = rng.uniform(r, w - r) as f32;
        out[4 * i + 1] = rng.uniform(r, h - r) as f32;
        out[4 * i + 2] = rng.uniform(-v, v) as f32;
        out[4 * i + 3] = rng.uniform(-v, v) as f32;
    }
    out
}

fn bounce_step(state: &mut [f32], dt: f64) {
    let r = BALL_RADIUS as f64;
    let w = VIEWPORT_W as f64;
    let h = VIEWPORT_H as f64;
    let mut i = 0;
    while i < state.len() {
        let mut x = state[i] as f64;
        let mut y = state[i + 1] as f64;
        let mut vx = state[i + 2] as f64;
        let mut vy = state[i + 3] as f64;
        x += vx * dt;
        y += vy * dt;
        if x < r {
            x = r;
            vx = -vx;
        } else if x > w - r {
            x = w - r;
            vx = -vx;
        }
        if y < r {
            y = r;
            vy = -vy;
        } else if y > h - r {
            y = h - r;
            vy = -vy;
        }
        state[i] = x as f32;
        state[i + 1] = y as f32;
        state[i + 2] = vx as f32;
        state[i + 3] = vy as f32;
        i += 4;
    }
}

// =============================================================================
// Minimal Theme — required by install_theme (the framework panics on
// render without one; see [[project_install_theme_required]]).
// =============================================================================

#[derive(Clone)]
struct EmptyTheme;

impl ThemeTokens for EmptyTheme {
    fn tokens(&self) -> Vec<TokenEntry> {
        Vec::new()
    }
}

// =============================================================================
// State
// =============================================================================

#[derive(Clone, Copy, PartialEq, Eq)]
enum TestMode {
    Bounce,
    /// `AV.animate(SpringTo)` per ball, re-kicked every ~500ms so the
    /// springs stay continuously running. The framework's clock owns
    /// the per-frame tick; the variant's rAF only handles re-kicks
    /// and frame logging.
    Springstorm,
    /// O(N²) all-pairs gravity + elastic collisions. Same imperative
    /// shape as Bounce — variant owns the rAF, calls AV.set() per
    /// ball — but per-frame work scales N² in compute. Tests whether
    /// wasm beats V8 on the math at scale.
    Nbody,
}

struct AnimStore {
    mode: TestMode,
    /// Bounce: `[x, y, vx, vy]` interleaved. Springstorm: one target_y
    /// per ball (the next position each spring is heading toward;
    /// updated on re-kick).
    state: Vec<f32>,
    xs: Vec<AnimatedValue<f32>>,
    ys: Vec<AnimatedValue<f32>>,
    refs: Vec<Ref<ViewHandle>>,
    n: usize,
    /// Variant's own rAF loop. Bounce: drives the sim. Springstorm:
    /// handles re-kicks + frame timing (the framework's clock owns
    /// the per-AV ticking separately).
    raf: Option<RafLoop>,
    js_log: Vec<f64>,
    frame_log: Vec<f64>,
    last_frame_ms: Option<f64>,
    /// Springstorm: wall-clock of last re-kick; gates the 500ms cadence.
    last_rekick_ms: Option<f64>,
    /// Springstorm: RNG advanced on every re-kick. Pre-seeded in setup
    /// so iter-to-iter runs from the same seed re-target identically.
    rekick_state: u32,
    count_sig: Option<Signal<u64>>,
}

impl AnimStore {
    fn empty() -> Self {
        Self {
            mode: TestMode::Bounce,
            state: Vec::new(),
            xs: Vec::new(),
            ys: Vec::new(),
            refs: Vec::new(),
            n: 0,
            raf: None,
            js_log: Vec::new(),
            frame_log: Vec::new(),
            last_frame_ms: None,
            last_rekick_ms: None,
            rekick_state: 0,
            count_sig: None,
        }
    }
}

// Spring storm constants — match idealyst's spring defaults (170/26/1.0).
const SPRING_REKICK_MS: f64 = 500.0;
const SPRING_TARGET_MIN: f64 = 50.0;
const SPRING_TARGET_MAX: f64 = (VIEWPORT_H as f64) - 50.0;

// N-body constants — match harness.js. Same f64-intermediate pattern
// as bounce (see [[project_cross_language_f64_constants]]) so the
// cross-language determinism check passes.
const NBODY_BALL_RADIUS: f32 = 6.0;
const NBODY_G: f64 = 800.0;
const NBODY_SOFTENING_SQ: f64 = 16.0;
const NBODY_MASS_MIN: f64 = 1.0;
const NBODY_MASS_MAX: f64 = 4.0;
const NBODY_VEL_RANGE: f64 = 30.0;

fn nbody_initial(n: usize, seed: u32) -> Vec<f32> {
    let r = NBODY_BALL_RADIUS as f64;
    let w = VIEWPORT_W as f64;
    let h = VIEWPORT_H as f64;
    let mut rng = Mulberry32::new(seed);
    let mut out = vec![0.0_f32; 5 * n];
    for i in 0..n {
        out[5 * i]     = rng.uniform(r, w - r) as f32;
        out[5 * i + 1] = rng.uniform(r, h - r) as f32;
        out[5 * i + 2] = rng.uniform(-NBODY_VEL_RANGE, NBODY_VEL_RANGE) as f32;
        out[5 * i + 3] = rng.uniform(-NBODY_VEL_RANGE, NBODY_VEL_RANGE) as f32;
        out[5 * i + 4] = rng.uniform(NBODY_MASS_MIN, NBODY_MASS_MAX) as f32;
    }
    out
}

/// Advance the N-body sim one fixed timestep. Iteration order MUST
/// match harness.js's `nbodyStep` byte-for-byte — FP summation order
/// is what makes the cross-language determinism check converge.
fn nbody_step(state: &mut [f32], dt: f64) {
    let r = NBODY_BALL_RADIUS as f64;
    let w = VIEWPORT_W as f64;
    let h = VIEWPORT_H as f64;
    let eps2 = NBODY_SOFTENING_SQ;
    let n = state.len() / 5;

    // f64 accumulators — matches JS's `new Float64Array(n)`.
    let mut ax = vec![0.0_f64; n];
    let mut ay = vec![0.0_f64; n];

    // O(N²) pair-force accumulation. Symmetric (Newton's 3rd law).
    for i in 0..n {
        let xi = state[5 * i] as f64;
        let yi = state[5 * i + 1] as f64;
        let mi = state[5 * i + 4] as f64;
        for j in (i + 1)..n {
            let dx = (state[5 * j] as f64) - xi;
            let dy = (state[5 * j + 1] as f64) - yi;
            let r2 = dx * dx + dy * dy + eps2;
            let inv_r3 = 1.0 / (r2 * r2.sqrt());
            let mj = state[5 * j + 4] as f64;
            let f = NBODY_G * inv_r3;
            ax[i] += f * mj * dx;
            ay[i] += f * mj * dy;
            ax[j] -= f * mi * dx;
            ay[j] -= f * mi * dy;
        }
    }

    // O(N²) elastic collisions. Same iteration order.
    let min_dist = 2.0 * r;
    let min_dist_sq = min_dist * min_dist;
    for i in 0..n {
        let mi = state[5 * i + 4] as f64;
        for j in (i + 1)..n {
            let dx = (state[5 * j] as f64) - (state[5 * i] as f64);
            let dy = (state[5 * j + 1] as f64) - (state[5 * i + 1] as f64);
            let d_sq = dx * dx + dy * dy;
            if d_sq >= min_dist_sq || d_sq == 0.0 {
                continue;
            }
            let d = d_sq.sqrt();
            let nx = dx / d;
            let ny = dy / d;
            let vxi = state[5 * i + 2] as f64;
            let vyi = state[5 * i + 3] as f64;
            let vxj = state[5 * j + 2] as f64;
            let vyj = state[5 * j + 3] as f64;
            let v_rel_n = (vxi - vxj) * nx + (vyi - vyj) * ny;
            if v_rel_n < 0.0 {
                continue;
            }
            let mj = state[5 * j + 4] as f64;
            let total_m = mi + mj;
            let j_imp = (2.0 * v_rel_n) / (1.0 / mi + 1.0 / mj);
            state[5 * i + 2] = (vxi - (j_imp / mi) * nx) as f32;
            state[5 * i + 3] = (vyi - (j_imp / mi) * ny) as f32;
            state[5 * j + 2] = (vxj + (j_imp / mj) * nx) as f32;
            state[5 * j + 3] = (vyj + (j_imp / mj) * ny) as f32;
            let overlap = min_dist - d;
            let push_i = overlap * (mj / total_m);
            let push_j = overlap * (mi / total_m);
            state[5 * i]     = ((state[5 * i] as f64) - nx * push_i) as f32;
            state[5 * i + 1] = ((state[5 * i + 1] as f64) - ny * push_i) as f32;
            state[5 * j]     = ((state[5 * j] as f64) + nx * push_j) as f32;
            state[5 * j + 1] = ((state[5 * j + 1] as f64) + ny * push_j) as f32;
        }
    }

    // Integrate + reflect. Reads collision-updated velocities from
    // state; reads accelerations from the local buffer.
    for i in 0..n {
        let mut x  = state[5 * i] as f64;
        let mut y  = state[5 * i + 1] as f64;
        let mut vx = state[5 * i + 2] as f64;
        let mut vy = state[5 * i + 3] as f64;
        vx += ax[i] * dt;
        vy += ay[i] * dt;
        x  += vx * dt;
        y  += vy * dt;
        if x < r { x = r; vx = -vx; }
        else if x > w - r { x = w - r; vx = -vx; }
        if y < r { y = r; vy = -vy; }
        else if y > h - r { y = h - r; vy = -vy; }
        state[5 * i]     = x as f32;
        state[5 * i + 1] = y as f32;
        state[5 * i + 2] = vx as f32;
        state[5 * i + 3] = vy as f32;
        // mass unchanged
    }
}

thread_local! {
    static STORE: RefCell<AnimStore> = RefCell::new(AnimStore::empty());
    static OWNER: RefCell<Option<Owner>> = const { RefCell::new(None) };
}

// =============================================================================
// Stylesheets — built once and `Rc::clone`d per ball
// =============================================================================

fn stage_sheet() -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(StyleRules {
        position: Some(Position::Relative),
        width: Some(Tokenized::Literal(Length::Px(VIEWPORT_W))),
        height: Some(Tokenized::Literal(Length::Px(VIEWPORT_H))),
        background: Some(Tokenized::Literal(Color("#14171c".into()))),
        border_top_left_radius:     Some(Tokenized::Literal(Length::Px(4.0))),
        border_top_right_radius:    Some(Tokenized::Literal(Length::Px(4.0))),
        border_bottom_left_radius:  Some(Tokenized::Literal(Length::Px(4.0))),
        border_bottom_right_radius: Some(Tokenized::Literal(Length::Px(4.0))),
        ..Default::default()
    }))
}

fn ball_sheet() -> Rc<StyleSheet> {
    // Negative top/left of `-r` places the visual at the top-left
    // origin so that a TranslateX/Y of (x, y) renders the BALL'S
    // CENTER at (x, y) — matches the vanilla variant's
    // `margin-left: -r; margin-top: -r` shape and keeps the cross-
    // variant state semantics aligned ("state.x/y is the ball
    // center, not its top-left corner").
    Rc::new(StyleSheet::r#static(StyleRules {
        position: Some(Position::Absolute),
        top: Some(Tokenized::Literal(Length::Px(-BALL_RADIUS))),
        left: Some(Tokenized::Literal(Length::Px(-BALL_RADIUS))),
        width: Some(Tokenized::Literal(Length::Px(BALL_RADIUS * 2.0))),
        height: Some(Tokenized::Literal(Length::Px(BALL_RADIUS * 2.0))),
        border_top_left_radius:     Some(Tokenized::Literal(Length::Px(BALL_RADIUS))),
        border_top_right_radius:    Some(Tokenized::Literal(Length::Px(BALL_RADIUS))),
        border_bottom_left_radius:  Some(Tokenized::Literal(Length::Px(BALL_RADIUS))),
        border_bottom_right_radius: Some(Tokenized::Literal(Length::Px(BALL_RADIUS))),
        background: Some(Tokenized::Literal(Color("#5b6cff".into()))),
        ..Default::default()
    }))
}

// =============================================================================
// App tree
// =============================================================================

fn app(count_sig: Signal<u64>) -> Primitive {
    install_theme(EmptyTheme);
    let ss = stage_sheet();
    let bs = ball_sheet();
    ui! {
        View(style = ss.clone()) {
            match count_sig.get() {
                _v => {
                    {
                        // Snapshot refs out of STORE on every rebuild. The
                        // match arm fires when count_sig changes; setup_anim
                        // writes the new refs to STORE BEFORE bumping the
                        // signal, so this read sees the current ref set.
                        let refs: Vec<Ref<ViewHandle>> =
                            STORE.with(|s| s.borrow().refs.clone());
                        let bs = bs.clone();
                        let children: Vec<Primitive> = refs
                            .iter()
                            .map(|r| ui! { View(style = bs.clone()) {}.bind(*r) })
                            .collect();
                        ui! { View { { children } } }
                    }
                }
            }
        }
    }
}

// =============================================================================
// JS-exported lifecycle
// =============================================================================

#[wasm_bindgen]
pub fn start() {
    console_error_panic_hook::set_once();
    // Both required before render — install_scheduler so rAF + microtask
    // dispatch land on real callbacks, install_time_source so the
    // framework's clock has a non-zero now_micros (per
    // [[project_web_bootstrap_scheduler]]).
    backend_web::install_scheduler();
    backend_web::install_time_source();
    backend_web::install_drop_deferral();

    let count_sig: Signal<u64> = signal!(0u64);
    STORE.with(|s| s.borrow_mut().count_sig = Some(count_sig));

    let backend = Rc::new(RefCell::new(WebBackend::new("#app")));
    // Required so `ViewHandle::set_animated_f32` (called from every
    // `AnimatedValue::bind` listener) can route through to the
    // backend. Without it, `WEB_BACKEND_HANDLE` stays None and every
    // animation write silently no-ops — see [[project_web_install_global_self_for_animation]].
    backend_web::install_global_self(&backend);
    let owner = render(backend, app(count_sig));
    OWNER.with(|s| *s.borrow_mut() = Some(owner));
}

#[wasm_bindgen]
pub fn setup_anim(test: &str, n: u32, seed: u32) {
    let mode = match test {
        "bounce" => TestMode::Bounce,
        "springstorm" => TestMode::Springstorm,
        "nbody" => TestMode::Nbody,
        _ => panic!("idealyst-native-anim: unknown test '{}'", test),
    };
    let n = n as usize;

    // Cancel any in-flight rAF + any framework-side animators from a
    // previous start. Otherwise the framework's clock would keep
    // ticking the OLD AVs after we swap them out; the new AVs would
    // also kick fresh ticks, doubling per-frame cost. For springstorm
    // specifically, we cancel via av.cancel() inside stop_anim — but
    // a sloppy caller might setup again without stopping, so do it
    // here too.
    STORE.with(|s| {
        let mut store = s.borrow_mut();
        store.raf = None;
        for av in &store.ys {
            av.cancel();
        }
    });

    // Bounce: state = [x, y, vx, vy] × n.
    // Springstorm: state = [target_y] × n; xs hold the fixed per-ball
    // X positions (set once, never animated again — bound to the
    // backend through the standard AV path).
    let (state, initial_xs, initial_ys): (Vec<f32>, Vec<f32>, Vec<f32>) = match mode {
        TestMode::Bounce => {
            let s = bounce_initial(n, seed);
            let xs = (0..n).map(|i| s[4 * i]).collect();
            let ys = (0..n).map(|i| s[4 * i + 1]).collect();
            (s, xs, ys)
        }
        TestMode::Nbody => {
            // Stride-5 layout — initial xs/ys read from [5*i, 5*i+1].
            let s = nbody_initial(n, seed);
            let xs = (0..n).map(|i| s[5 * i]).collect();
            let ys = (0..n).map(|i| s[5 * i + 1]).collect();
            (s, xs, ys)
        }
        TestMode::Springstorm => {
            let mut rng = Mulberry32::new(seed);
            let mut targets = vec![0.0_f32; n];
            let mut initial_ys = vec![0.0_f32; n];
            for i in 0..n {
                initial_ys[i] = rng.uniform(SPRING_TARGET_MIN, SPRING_TARGET_MAX) as f32;
                targets[i] = rng.uniform(SPRING_TARGET_MIN, SPRING_TARGET_MAX) as f32;
            }
            // Fixed X grid per ball — same shape as vanilla springstorm.
            let xs = (0..n).map(|i| ((i * 7) % (VIEWPORT_W as usize)) as f32).collect();
            (targets, xs, initial_ys)
        }
    };

    // Fresh refs + AVs. Old AVs / subscriptions from previous setups
    // leak (`mem::forget` inside `.bind()`); see
    // crates/framework/core/src/animation/binding.rs. Tolerable for
    // benchmark setup churn.
    let refs: Vec<Ref<ViewHandle>> = (0..n).map(|_| node_ref!(ViewHandle)).collect();
    let xs: Vec<AnimatedValue<f32>> =
        (0..n).map(|i| AnimatedValue::new(initial_xs[i])).collect();
    let ys: Vec<AnimatedValue<f32>> =
        (0..n).map(|i| AnimatedValue::new(initial_ys[i])).collect();
    for i in 0..n {
        xs[i].bind(refs[i], AnimProp::TranslateX);
        ys[i].bind(refs[i], AnimProp::TranslateY);
    }

    let count_sig = STORE.with(|s| {
        let mut store = s.borrow_mut();
        store.mode = mode;
        store.state = state;
        store.xs = xs;
        store.ys = ys;
        store.refs = refs;
        store.n = n;
        store.js_log.clear();
        store.frame_log.clear();
        store.last_frame_ms = None;
        store.last_rekick_ms = None;
        store.rekick_state = seed;
        store.count_sig.expect("start() must run before setup_anim()")
    });
    count_sig.update(|v| *v = v.wrapping_add(1));
}

#[wasm_bindgen]
pub fn step_to(frame_n: u32) {
    STORE.with(|s| {
        let mut store = s.borrow_mut();
        let mode = store.mode;
        for _ in 0..frame_n {
            match mode {
                TestMode::Bounce => bounce_step(&mut store.state, FIXED_DT),
                TestMode::Nbody => nbody_step(&mut store.state, FIXED_DT),
                // Springstorm has no deterministic stepTo — its physics
                // is the framework's spring integrator, which runs
                // off the clock. Suite skips determinism for spring
                // suites so this branch is unreachable in practice.
                TestMode::Springstorm => {}
            }
        }
        // Push the post-step state into the AVs too. Determinism check
        // only reads `get_state()` so this is technically optional, but
        // symmetry with the perf path keeps the rendered tree
        // consistent with simulated state after step_to.
        let n = store.n;
        let (sx, sy) = stride_offsets(mode);
        for i in 0..n {
            let x = store.state[sx + i * stride(mode)];
            let y = store.state[sy + i * stride(mode)];
            store.xs[i].set(x);
            store.ys[i].set(y);
        }
    });
}

/// Stride between consecutive bodies in the `state` buffer for each
/// test mode. Bounce: 4 (x,y,vx,vy). Nbody: 5 (x,y,vx,vy,mass).
/// Springstorm: 1 (single target_y per ball).
fn stride(mode: TestMode) -> usize {
    match mode {
        TestMode::Bounce => 4,
        TestMode::Nbody => 5,
        TestMode::Springstorm => 1,
    }
}

/// Offsets of `x` and `y` within a single body's stride. Bounce + nbody:
/// (0, 1). Springstorm: state is just target_y so x/y unused via this
/// path.
fn stride_offsets(mode: TestMode) -> (usize, usize) {
    match mode {
        TestMode::Bounce | TestMode::Nbody => (0, 1),
        TestMode::Springstorm => (0, 0),
    }
}

/// Return the current sim state as `Vec<f32>`. wasm-bindgen marshals
/// this as a `Float32Array` on the JS side, which is exactly what the
/// harness's `assertReference` expects.
#[wasm_bindgen]
pub fn get_state() -> Vec<f32> {
    STORE.with(|s| s.borrow().state.clone())
}

fn perf() -> Performance {
    web_sys::window()
        .and_then(|w| w.performance())
        .expect("Performance API unavailable")
}

#[wasm_bindgen]
pub fn start_anim() {
    let mode = STORE.with(|s| {
        let mut store = s.borrow_mut();
        store.js_log.clear();
        store.frame_log.clear();
        store.last_frame_ms = None;
        store.last_rekick_ms = None;
        store.raf = None;
        store.mode
    });

    if mode == TestMode::Springstorm {
        // Kick every AV's spring once. The framework's clock takes over
        // ticking from here; our own rAF below only handles re-kicks +
        // frame timing.
        STORE.with(|s| {
            let store = s.borrow();
            let n = store.n;
            for i in 0..n {
                let target = store.state[i];
                store.ys[i].animate(SpringTo::new(target));
            }
        });
    }

    let p = perf();
    let raf = framework_core::raf_loop(move || {
        let now = p.now();
        STORE.with(|s| {
            let mut store = s.borrow_mut();
            // Frame interval — captured BEFORE work so a hitch in this
            // frame is attributed to NEXT frame's frameDt (matches
            // vanilla + the rebuild suite's worstFrame attribution).
            if let Some(last) = store.last_frame_ms {
                let dt = now - last;
                if dt > 0.0 {
                    store.frame_log.push(dt);
                }
            }
            store.last_frame_ms = Some(now);

            let t0 = p.now();
            match store.mode {
                TestMode::Bounce => bounce_tick(&mut store),
                TestMode::Nbody => nbody_tick(&mut store),
                TestMode::Springstorm => springstorm_tick(&mut store, now),
            }
            let t1 = p.now();
            store.js_log.push(t1 - t0);
        });
    });
    STORE.with(|s| s.borrow_mut().raf = Some(raf));
}

/// Bounce: integrate the sim and push every ball's new (x, y) into its
/// AV. This is the full per-frame variant work for bounce.
fn bounce_tick(store: &mut AnimStore) {
    bounce_step(&mut store.state, FIXED_DT);
    let n = store.n;
    for i in 0..n {
        let x = store.state[4 * i];
        let y = store.state[4 * i + 1];
        store.xs[i].set(x);
        store.ys[i].set(y);
    }
}

/// N-body: O(N²) gravity + collisions, then write the new (x, y) of
/// each body. Compute-dominated above ~200 bodies (where the N² term
/// outpaces the linear write cost).
fn nbody_tick(store: &mut AnimStore) {
    nbody_step(&mut store.state, FIXED_DT);
    let n = store.n;
    for i in 0..n {
        let x = store.state[5 * i];
        let y = store.state[5 * i + 1];
        store.xs[i].set(x);
        store.ys[i].set(y);
    }
}

/// Springstorm: variant rAF only handles re-kicks (every ~500ms);
/// the framework's clock owns the per-frame spring ticking. `js_log`
/// for springstorm will be near zero — that's intentional and
/// documented in benchmark/anim/spec.md (use FPS / MAX ms as
/// headline metrics for this suite, not µs/FRAME for idealyst).
fn springstorm_tick(store: &mut AnimStore, now_ms: f64) {
    let due = match store.last_rekick_ms {
        Some(last) => now_ms - last >= SPRING_REKICK_MS,
        None => true,
    };
    if !due {
        return;
    }
    store.last_rekick_ms = Some(now_ms);

    let n = store.n;
    let mut rng = Mulberry32::new(store.rekick_state);
    for i in 0..n {
        let target = rng.uniform(SPRING_TARGET_MIN, SPRING_TARGET_MAX) as f32;
        store.state[i] = target;
        // animate() replaces the previous animator; the AV's existing
        // clock registration stays live, so we're not paying a
        // tick-register churn cost per re-kick. Velocity hands off
        // from the previous spring (default behavior).
        store.ys[i].animate(SpringTo::new(target));
    }
    // Advance the RNG state so the next re-kick draws different
    // targets. Saving back to the store; the integer roundtrip is
    // cheap.
    store.rekick_state = rng.state();
}

/// Cancel the rAF loop and return `{ jsPerFrame, frameDt }` as a JSON
/// string for the JS suite to parse. Hand-rolled formatter to avoid
/// pulling serde into this crate — the data shape is fixed (two number
/// arrays) so the encoder is trivial.
///
/// Also cancels every AV's animator. For bounce this is a no-op (we
/// don't install animators, we call `.set()`); for springstorm it
/// stops the framework's clock from continuing to tick the springs
/// after our sample window closes — otherwise the next setup_anim
/// would see stale per-AV ticks still running.
#[wasm_bindgen]
pub fn stop_anim() -> String {
    STORE.with(|s| {
        let mut store = s.borrow_mut();
        store.raf = None;
        for av in &store.ys {
            av.cancel();
        }
        let js = std::mem::take(&mut store.js_log);
        let frame = std::mem::take(&mut store.frame_log);
        format_log_json(&js, &frame)
    })
}

fn format_log_json(js: &[f64], frame: &[f64]) -> String {
    let mut out = String::with_capacity((js.len() + frame.len()) * 8 + 64);
    out.push_str("{\"jsPerFrame\":[");
    push_floats(&mut out, js);
    out.push_str("],\"frameDt\":[");
    push_floats(&mut out, frame);
    out.push_str("]}");
    out
}

fn push_floats(buf: &mut String, xs: &[f64]) {
    use std::fmt::Write;
    for (i, v) in xs.iter().enumerate() {
        if i > 0 {
            buf.push(',');
        }
        // 3 decimal places → sub-millisecond, ample for benchmark
        // analysis. Skip Display's default precision (which would emit
        // 17-digit float strings).
        let _ = write!(buf, "{:.3}", v);
    }
}
