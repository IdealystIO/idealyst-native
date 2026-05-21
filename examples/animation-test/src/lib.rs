//! Animation-system smoke test.
//!
//! Single platform-agnostic crate. [`app`] returns a `Primitive` tree
//! that exercises every built-in animator and composition primitive
//! through the `AnimatedValue` → `Signal` → reactive-style bridge.
//!
//! # What each card demonstrates
//!
//! 1. **Spring tap** (purple) — tap to fire `SpringTo::new(1.1)` then
//!    spring back to 1.0 on release. The second tap mid-animation
//!    exercises velocity-preserving handoff.
//! 2. **Decay drag** (orange) — pan horizontally; on release, decay
//!    from the measured throw velocity. Exercises `DecayFrom` +
//!    gesture-driven `set` updates.
//! 3. **Loop pulse** (teal) — perpetual two-segment sequence inside a
//!    `Repeat::Forever` loop. Exercises `LoopFactory` +
//!    `SequenceFactory` + the clock's idle behaviour (this one keeps
//!    the clock alive; if you cancel it the clock drops the raf
//!    handle).
//! 4. **Keyframes bounce** (pink) — tap to fire a 3-stop bounce
//!    curve. Exercises `KeyframesTo`.
//! 5. **Stagger row** (5 mint cards) — tap "Reveal" to stagger five
//!    cards in via the `stagger(...)` helper.
//!
//! # Bridging AnimatedValue → backend
//!
//! Each card uses the same pattern:
//!
//! ```ignore
//! let av = AnimatedValue::new(initial);
//! let signal: Signal<f32> = signal!(initial);
//! bind_av_to_signal(&av, signal);
//! // reactive style closure reads `signal.get()`
//! ```
//!
//! [`bind_av_to_signal`] uses `subscribe_and_apply` so the signal
//! is consistent before the first tick. The returned `Subscription`
//! is intentionally leaked — the page is the test's lifetime; a
//! real app would tie it to a scope or component.

#[cfg(target_arch = "wasm32")]
mod web;

use framework_core::animation::{
    stagger, AnimProp, AnimatedValue, DecayFrom, KeyframesTo, LoopFactory, Repeat,
    SequenceFactory, SpringTo, TweenTo,
};
use framework_core::primitives::slider::slider;
use framework_core::{
    pan, signal, tap, text, view, AlignItems, Color, Easing, Effect, FlexDirection,
    JustifyContent, Length, Overflow, PanEvent, PanRecognizer, Position, Primitive, Ref, Signal,
    StyleApplication, StyleRules, StyleSheet, TapRecognizer, Tokenized, TouchEvent, TouchPhase,
    TouchResponse, ViewHandle,
};
use std::cell::RefCell;
use idea_ui::{install_theme, ThemeTokens, TokenEntry};
use std::rc::Rc;
use std::time::Duration;

// =============================================================================
// Theme stub — `install_theme` is required before render even when
// nothing reads tokens.
// =============================================================================

struct EmptyTheme;
impl ThemeTokens for EmptyTheme {
    fn tokens(&self) -> Vec<TokenEntry> {
        Vec::new()
    }
}

// =============================================================================
// App root
// =============================================================================

pub fn app() -> Primitive {
    install_theme(EmptyTheme);

    // Two-column body below the header:
    //  - Left:  the animated cards (spring / decay / loop / keyframes
    //           / stagger). Grows to take the remaining width so the
    //           card boxes still get a comfortable line length.
    //  - Right: the particle sim. Keeps its intrinsic 320 px field
    //           width (see `SIM_WIDTH`) so the physics doesn't have to
    //           reflow if the page resizes.
    let animated_col = view(vec![
        spring_tap_card().into(),
        decay_drag_card().into(),
        loop_pulse_card().into(),
        keyframes_bounce_card().into(),
        stagger_row_section().into(),
    ])
    .with_style(animated_col_sheet());

    let content_row = view(vec![
        animated_col.into(),
        particle_sim_section().into(),
    ])
    .with_style(content_row_sheet());

    view(vec![
        title_text("Animation Tests").into(),
        content_row.into(),
    ])
    .with_style(body_sheet())
    .into()
}

// =============================================================================
// Card 1 — Spring tap. Tap to scale up; tap again to spring back.
// =============================================================================

fn spring_tap_card() -> Primitive {
    let scale = AnimatedValue::new(1.0_f32);
    let view_ref: Ref<ViewHandle> = Ref::new();
    drive_scale(&scale, view_ref);

    let pressed = Rc::new(std::cell::Cell::new(false));

    let tap_scale = scale.clone();
    let tap_pressed = pressed.clone();
    let tap_handler = tap(TapRecognizer::new(), move || {
        let now_pressed = !tap_pressed.get();
        tap_pressed.set(now_pressed);
        let target = if now_pressed { 1.18 } else { 1.0 };
        log(&format!("[spring tap] firing — target={}", target));
        tap_scale.animate(SpringTo::new(target).stiffness(280.0).damping(20.0));
    });

    view(vec![
        text("Spring tap — toggles between rest and pressed via SpringTo")
            .with_style(card_caption_sheet())
            .into(),
    ])
    .with_style(card_box_sheet("#7a3aff"))
    .on_touch(move |ev| tap_handler(ev))
    .bind(view_ref)
    .into()
}

// =============================================================================
// Card 2 — Decay drag. Pan horizontally; release decays from velocity.
// =============================================================================

fn decay_drag_card() -> Primitive {
    let translate = AnimatedValue::new(0.0_f32);
    let view_ref: Ref<ViewHandle> = Ref::new();
    drive_translate_x(&translate, view_ref);

    // Snapshot of `translate` at gesture start. Pan deltas are
    // relative to Began; we add to the cached rest offset so a
    // mid-decay drag picks up where the decay is *right now*.
    let drag_start: Signal<f32> = signal!(0.0);

    let drag_av = translate.clone();
    let start = drag_start;
    let handler = pan(PanRecognizer::new(), move |ev| match ev {
        PanEvent::Began { .. } => {
            log("[decay] pan Began");
            start.set(drag_av.get());
            drag_av.cancel();
        }
        PanEvent::Moved { delta, .. } => {
            drag_av.set(start.get() + delta.x);
        }
        PanEvent::Ended { velocity } => {
            log(&format!("[decay] pan Ended v={:.1}", velocity.x));
            drag_av.animate(DecayFrom::new(velocity.x).friction(3.5));
        }
        PanEvent::Cancelled => {
            log("[decay] pan Cancelled");
            drag_av.animate(SpringTo::new(0.0).stiffness(140.0).damping(20.0));
        }
    });

    view(vec![
        text("Decay drag — fling horizontally, decay friction settles it")
            .with_style(card_caption_sheet())
            .into(),
    ])
    .with_style(card_box_sheet("#ff9933"))
    .on_touch(move |ev| handler(ev))
    .bind(view_ref)
    .into()
}

// =============================================================================
// Card 3 — Loop pulse. Forever-looping two-segment sequence.
// =============================================================================

fn loop_pulse_card() -> Primitive {
    let scale = AnimatedValue::new(1.0_f32);
    let view_ref: Ref<ViewHandle> = Ref::new();
    drive_scale(&scale, view_ref);

    // Kick off the pulse on first mount. `Box::leak` keeps the
    // value handle (and its tick registration) alive for the page's
    // lifetime; a real app would tie this to a scope.
    let pulse = SequenceFactory::<f32>::new()
        .then(TweenTo::new(1.08_f32, Duration::from_millis(450)).ease_in_out())
        .then(TweenTo::new(1.0_f32, Duration::from_millis(450)).ease_in_out());
    scale.animate(LoopFactory::new(pulse, Repeat::Forever));
    let _ = Box::leak(Box::new(scale));

    view(vec![
        text("Loop pulse — Sequence of two tweens inside Repeat::Forever")
            .with_style(card_caption_sheet())
            .into(),
    ])
    .with_style(card_box_sheet("#1abc9c"))
    .bind(view_ref)
    .into()
}

// =============================================================================
// Card 4 — Keyframes bounce. Three-stop curve.
// =============================================================================

fn keyframes_bounce_card() -> Primitive {
    let scale = AnimatedValue::new(1.0_f32);
    let view_ref: Ref<ViewHandle> = Ref::new();
    drive_scale(&scale, view_ref);

    let tap_scale = scale.clone();
    let tap_handler = tap(TapRecognizer::new(), move || {
        log("[keyframes] firing bounce");
        tap_scale.animate(
            KeyframesTo::new(Duration::from_millis(480))
                .stop(0.0, 1.0_f32)
                .stop(0.4, 1.22)
                .stop(0.7, 0.94)
                .stop(1.0, 1.0)
                .curve(Easing::EaseInOut),
        );
    });

    view(vec![
        text("Keyframes bounce — KeyframesTo with 4 stops")
            .with_style(card_caption_sheet())
            .into(),
    ])
    .with_style(card_box_sheet("#ff4d8d"))
    .on_touch(move |ev| tap_handler(ev))
    .bind(view_ref)
    .into()
}

// =============================================================================
// Section — Stagger row. Tap "Reveal" to stagger 5 cards in.
// =============================================================================

const STAGGER_COUNT: usize = 5;
const STAGGER_STEP_MS: u64 = 60;

fn stagger_row_section() -> Primitive {
    // Each chip gets its own AnimatedValue<f32> driving translateX
    // (offscreen at -240 → settled at 0) and its own ViewHandle ref
    // for per-frame inline writes.
    let chips: Vec<(AnimatedValue<f32>, Ref<ViewHandle>)> = (0..STAGGER_COUNT)
        .map(|_| {
            let av = AnimatedValue::new(-240.0_f32);
            let view_ref: Ref<ViewHandle> = Ref::new();
            drive_translate_x(&av, view_ref);
            (av, view_ref)
        })
        .collect();

    // Build the row of chip Views before we move out of `chips`.
    let chip_views: Vec<Primitive> = chips
        .iter()
        .map(|(_, view_ref)| {
            view(vec![])
                .with_style(chip_box_sheet("#11ddaa"))
                .bind(*view_ref)
                .into()
        })
        .collect();

    let reveal_avs: Vec<AnimatedValue<f32>> =
        chips.into_iter().map(|(av, _)| av).collect();
    let reveal_handler = tap(TapRecognizer::new(), move || {
        log("[stagger] reveal");
        for av in &reveal_avs {
            av.set(-240.0);
        }
        stagger(
            &reveal_avs,
            Duration::from_millis(STAGGER_STEP_MS),
            |_i| SpringTo::new(0.0_f32).stiffness(220.0).damping(22.0),
        );
    });

    let reveal_button: Primitive = view(vec![text("Reveal")
        .with_style(button_label_sheet())
        .into()])
    .with_style(button_sheet())
    .on_touch(move |ev| reveal_handler(ev))
    .into();

    view(vec![
        text("Stagger — tap Reveal to spring 5 chips in")
            .with_style(card_caption_sheet())
            .into(),
        view(chip_views).with_style(chip_row_sheet()).into(),
        reveal_button,
    ])
    .with_style(stagger_section_sheet())
    .into()
}

// =============================================================================
// Particle sim — 2D elastic collisions + draggable ball
// =============================================================================
//
// A `ParticleSim` owns a `Vec<Particle>`; the per-frame `step` advances
// every particle, resolves wall + pair-wise collisions, then writes the
// new (x, y) to each particle's mounted node via the backend's
// `set_animated_f32` fast path. No `AnimatedValue` per particle — the
// values *are* the simulation state, and the per-frame tick is hooked
// directly into `animation::clock::register_guarded`.
//
// The big ball is just another particle with larger mass and radius.
// A pan recognizer on its node updates the particle's position
// (during drag) and velocity (on release), letting the standard
// elastic-collision code carry the throw into the rest of the sim.

const SIM_WIDTH: f32 = 320.0;
const SIM_HEIGHT: f32 = 260.0;
/// Initial particles seeded on a ring at app start.
const N_SMALL: usize = 14;
/// Pool size for particles. The first slot is reserved for the big
/// ball, the next `N_SMALL` are the seeded mints, the rest stay
/// inactive until the user adds them via the "Add particle" mode.
/// Pre-mounting the views avoids needing reactive children to grow
/// the particle count at runtime.
const MAX_PARTICLES: usize = 64;
/// Pool size for walls. Same posture as particles — pre-mounted
/// hidden, activated on demand.
const MAX_WALLS: usize = 16;
const R_SMALL: f32 = 11.0;
const R_BIG: f32 = 24.0;
const M_SMALL: f32 = 1.0;
const M_BIG: f32 = 6.0;
const MAX_STEP_S: f32 = 0.032;
/// Base render size of every particle / wall view in pixels. The
/// runtime sizes them via `transform: scale(...)` so a single
/// stylesheet covers the whole pool — see [`Particle::publish`].
const BASE_SIZE_PX: f32 = 100.0;
const BASE_HALF: f32 = BASE_SIZE_PX / 2.0;

/// What the canvas does with a touch. Switched by the toolbar
/// buttons; read by the sim's touch handler on every event so a
/// mid-gesture mode flip cancels cleanly on the next pointer up.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    /// Drag anywhere → throw the violet ball.
    Throw,
    /// Drag to draw an axis-aligned wall rectangle.
    DrawWall,
    /// Tap to spawn a particle; drag to spawn one with a throw
    /// velocity proportional to the drag delta.
    AddParticle,
}

struct Particle {
    /// Centre position in canvas-relative pixels.
    position: (f32, f32),
    /// Velocity in pixels-per-second.
    velocity: (f32, f32),
    mass: f32,
    radius: f32,
    /// `false` slots are off-canvas placeholders waiting to be
    /// activated by [`ParticleSim::spawn_particle`]. The pool is
    /// pre-mounted so the renderer doesn't have to reactively add
    /// children when the user spawns one.
    active: bool,
    /// `true` while the user is mid-drag in `Add particle` mode.
    /// Preview particles render normally but are skipped by the
    /// physics step (no integration, no collisions) — the cursor
    /// is driving their position directly. On touch release the
    /// flag clears and they fall back into the simulation with
    /// the gesture's accumulated velocity.
    preview: bool,
    /// Special big-mass ball — exempt from physics-driven motion
    /// while the user is dragging it (the drag handler sets the
    /// position directly).
    is_ball: bool,
    /// View ref filled by the walker on mount. Once set, the per-
    /// frame writer downcasts to `web_sys::Node` (wasm only) and
    /// pushes `translate` + `scale` updates inline.
    view_ref: Ref<ViewHandle>,
}

struct Wall {
    /// Top-left corner + size, all in canvas pixels.
    rect: (f32, f32, f32, f32),
    active: bool,
    view_ref: Ref<ViewHandle>,
}

struct ParticleSim {
    particles: Vec<Particle>,
    walls: Vec<Wall>,
    width: f32,
    height: f32,
    /// Index of the user-controllable big ball within `particles`.
    /// Held during a drag so the pan handler can mutate just that
    /// entry without re-scanning.
    ball_index: usize,
    /// 0..=1. Energy retained on each bounce. Wired to the
    /// "Bounce" slider in the toolbar.
    restitution: f32,
    /// 0..∞ (inverse seconds). Per-frame velocity is multiplied
    /// by `exp(-drag * dt)`. Wired to the "Drag" slider.
    drag: f32,
}

impl ParticleSim {
    fn new(width: f32, height: f32) -> Self {
        Self {
            particles: Vec::new(),
            walls: Vec::new(),
            width,
            height,
            ball_index: usize::MAX,
            restitution: 0.96,
            drag: 0.0,
        }
    }

    fn push(&mut self, p: Particle) {
        self.particles.push(p);
    }

    /// Activate an inactive particle slot, returning its index.
    /// Returns `None` if the pool is full — the caller drops the
    /// add silently rather than panicking.
    ///
    /// Kept around even though the demo currently spawns via
    /// `spawn_preview_particle` + `commit_preview` — the one-shot
    /// shape stays useful for scripted spawns (gravity wells,
    /// emitters, etc.) that don't go through a gesture.
    #[allow(dead_code)]
    fn spawn_particle(&mut self, position: (f32, f32), velocity: (f32, f32)) -> Option<usize> {
        let idx = self
            .particles
            .iter()
            .position(|p| !p.active && !p.is_ball)?;
        let p = &mut self.particles[idx];
        p.position = position;
        p.velocity = velocity;
        p.mass = M_SMALL;
        p.radius = R_SMALL;
        p.active = true;
        p.preview = false;
        Some(idx)
    }

    /// Spawn a *preview* particle. Same shape as a real one, just
    /// flagged out of the physics loop — see [`Particle::preview`].
    /// Used by the `Add particle` mode while the user is still
    /// dragging: the cursor drives the position and the gesture's
    /// instantaneous velocity is accumulated; on release the
    /// caller flips `preview` off and the particle joins the sim.
    fn spawn_preview_particle(&mut self, position: (f32, f32)) -> Option<usize> {
        let idx = self
            .particles
            .iter()
            .position(|p| !p.active && !p.is_ball)?;
        let p = &mut self.particles[idx];
        p.position = position;
        p.velocity = (0.0, 0.0);
        p.mass = M_SMALL;
        p.radius = R_SMALL;
        p.active = true;
        p.preview = true;
        Some(idx)
    }

    /// Promote a preview slot into a real particle: copy the
    /// gesture-measured velocity in and clear the preview flag.
    /// No-op if the slot isn't currently a preview (e.g. the
    /// gesture was cancelled and the slot already cleared).
    fn commit_preview(&mut self, slot: usize, velocity: (f32, f32)) {
        if let Some(p) = self.particles.get_mut(slot) {
            if p.preview {
                p.velocity = velocity;
                p.preview = false;
            }
        }
    }

    /// Cancel a preview slot back to inactive. Called on touch
    /// cancellation — the user lifted off-screen, or the gesture
    /// was preempted.
    fn cancel_preview(&mut self, slot: usize) {
        if let Some(p) = self.particles.get_mut(slot) {
            if p.preview {
                p.active = false;
                p.preview = false;
            }
        }
    }

    /// Direct-write the preview slot's position. Bypasses physics —
    /// the cursor is driving where the ghost sits while the user
    /// is mid-drag.
    fn set_preview_position(&mut self, slot: usize, position: (f32, f32)) {
        if let Some(p) = self.particles.get_mut(slot) {
            if p.preview {
                p.position = position;
            }
        }
    }

    /// Activate (or update) a wall slot. If `slot` is `Some`, that
    /// specific slot is reused — used by the draw-wall preview to
    /// keep the same view growing as the gesture extends. If
    /// `None`, the first inactive slot is grabbed.
    fn put_wall(&mut self, slot: Option<usize>, rect: (f32, f32, f32, f32)) -> Option<usize> {
        let idx = match slot {
            Some(i) if i < self.walls.len() => i,
            Some(_) => return None,
            None => self.walls.iter().position(|w| !w.active)?,
        };
        let w = &mut self.walls[idx];
        w.rect = rect;
        w.active = true;
        Some(idx)
    }

    fn step(&mut self, dt: std::time::Duration) {
        // Cap the slice — long pauses (system sleep, tab background)
        // would otherwise teleport the particles past their walls
        // and through each other in one step.
        let dt_s = (dt.as_secs_f32()).min(MAX_STEP_S);

        // 0. Air friction. Multiply every velocity by `exp(-drag *
        // dt)`. With drag = 0 this is a no-op; positive values cool
        // the sim toward rest. Closed-form so frame-rate doesn't
        // change the qualitative behaviour.
        if self.drag > 0.0 {
            let decay = (-self.drag * dt_s).exp();
            for p in self.particles.iter_mut().filter(|p| p.active && !p.preview) {
                p.velocity.0 *= decay;
                p.velocity.1 *= decay;
            }
        }

        // 1. Integrate position.
        for p in self.particles.iter_mut().filter(|p| p.active && !p.preview) {
            p.position.0 += p.velocity.0 * dt_s;
            p.position.1 += p.velocity.1 * dt_s;
        }

        // 2. Bounce off the canvas edges. Restitution damps each
        // bounce (slider-controlled; at 1.0 the system runs
        // forever, at <1 it slowly cools).
        let r_coef = self.restitution;
        for p in self.particles.iter_mut().filter(|p| p.active && !p.preview) {
            let r = p.radius;
            if p.position.0 < r {
                p.position.0 = r;
                p.velocity.0 = p.velocity.0.abs() * r_coef;
            } else if p.position.0 > self.width - r {
                p.position.0 = self.width - r;
                p.velocity.0 = -p.velocity.0.abs() * r_coef;
            }
            if p.position.1 < r {
                p.position.1 = r;
                p.velocity.1 = p.velocity.1.abs() * r_coef;
            } else if p.position.1 > self.height - r {
                p.position.1 = self.height - r;
                p.velocity.1 = -p.velocity.1.abs() * r_coef;
            }
        }

        // 3. Resolve collisions with user-drawn walls. For each
        // (particle, wall) pair: find the closest point on the
        // axis-aligned rect to the particle's centre; if within
        // radius, reflect the particle's velocity across the
        // normal pointing from the contact point to the centre.
        // Walls are static (infinite mass), so the impulse is just
        // `v -= (1+e) * (v · n) * n`.
        let n_walls = self.walls.len();
        for p in self.particles.iter_mut().filter(|p| p.active && !p.preview) {
            for w_idx in 0..n_walls {
                let w = &self.walls[w_idx];
                if !w.active {
                    continue;
                }
                let (wx, wy, ww, wh) = w.rect;
                let cx = p.position.0.clamp(wx, wx + ww);
                let cy = p.position.1.clamp(wy, wy + wh);
                let dx = p.position.0 - cx;
                let dy = p.position.1 - cy;
                let dist_sq = dx * dx + dy * dy;
                let r = p.radius;
                if dist_sq < r * r {
                    if dist_sq > 0.0 {
                        let dist = dist_sq.sqrt();
                        let nx = dx / dist;
                        let ny = dy / dist;
                        // Push out so the particle sits flush.
                        let overlap = r - dist;
                        p.position.0 += nx * overlap;
                        p.position.1 += ny * overlap;
                        let vn = p.velocity.0 * nx + p.velocity.1 * ny;
                        if vn < 0.0 {
                            let j_mag = -(1.0 + r_coef) * vn;
                            p.velocity.0 += j_mag * nx;
                            p.velocity.1 += j_mag * ny;
                        }
                    } else {
                        // Centre is exactly on the wall edge —
                        // pick the nearer axis to push out along.
                        let to_left = p.position.0 - wx;
                        let to_right = wx + ww - p.position.0;
                        let to_top = p.position.1 - wy;
                        let to_bot = wy + wh - p.position.1;
                        let min = to_left.min(to_right).min(to_top).min(to_bot);
                        if min == to_left {
                            p.position.0 = wx - r;
                            p.velocity.0 = -p.velocity.0.abs() * r_coef;
                        } else if min == to_right {
                            p.position.0 = wx + ww + r;
                            p.velocity.0 = p.velocity.0.abs() * r_coef;
                        } else if min == to_top {
                            p.position.1 = wy - r;
                            p.velocity.1 = -p.velocity.1.abs() * r_coef;
                        } else {
                            p.position.1 = wy + wh + r;
                            p.velocity.1 = p.velocity.1.abs() * r_coef;
                        }
                    }
                }
            }
        }

        // 4. Pair-wise elastic collisions between active particles.
        let n = self.particles.len();
        for i in 0..n {
            if !self.particles[i].active || self.particles[i].preview {
                continue;
            }
            for j in (i + 1)..n {
                if !self.particles[j].active || self.particles[j].preview {
                    continue;
                }
                let (left, right) = self.particles.split_at_mut(j);
                let pi = &mut left[i];
                let pj = &mut right[0];

                let dx = pj.position.0 - pi.position.0;
                let dy = pj.position.1 - pi.position.1;
                let sum_r = pi.radius + pj.radius;
                let dist_sq = dx * dx + dy * dy;
                if dist_sq >= sum_r * sum_r || dist_sq == 0.0 {
                    continue;
                }
                let dist = dist_sq.sqrt();
                let nx = dx / dist;
                let ny = dy / dist;

                let inv_mi = 1.0 / pi.mass;
                let inv_mj = 1.0 / pj.mass;

                let dvx = pj.velocity.0 - pi.velocity.0;
                let dvy = pj.velocity.1 - pi.velocity.1;
                let vn = dvx * nx + dvy * ny;
                if vn < 0.0 {
                    let j_mag = -(1.0 + r_coef) * vn / (inv_mi + inv_mj);
                    pi.velocity.0 -= j_mag * inv_mi * nx;
                    pi.velocity.1 -= j_mag * inv_mi * ny;
                    pj.velocity.0 += j_mag * inv_mj * nx;
                    pj.velocity.1 += j_mag * inv_mj * ny;
                }

                let overlap = sum_r - dist;
                let total_inv = inv_mi + inv_mj;
                let pi_share = overlap * inv_mi / total_inv;
                let pj_share = overlap * inv_mj / total_inv;
                pi.position.0 -= nx * pi_share;
                pi.position.1 -= ny * pi_share;
                pj.position.0 += nx * pj_share;
                pj.position.1 += ny * pj_share;
            }
        }

        // 5. Push the new positions to each node. Particles + walls
        // are sized via `scale(...)` on a 100×100 base element, so
        // every view in the pool ships with the same stylesheet and
        // sizing is purely a runtime transform.
        self.publish();
    }

    fn publish(&self) {
        for p in &self.particles {
            let _ = p.view_ref.with(|handle| {
                #[cfg(target_arch = "wasm32")]
                {
                    if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                        if !p.active {
                            // Hide inactive slots: scale to 0 so
                            // they collapse to a point.
                            crate::web::set_animated_f32(node, AnimProp::Scale, 0.0);
                            return;
                        }
                        // The base view is 100×100 with `transform-origin`
                        // at its centre (50, 50). We want the *visual*
                        // centre at `position` — so translate by
                        // `position - half` and scale to `radius / 50`
                        // (since the base half-extent is 50 px).
                        let scale = p.radius / BASE_HALF;
                        crate::web::set_animated_f32(
                            node,
                            AnimProp::TranslateX,
                            p.position.0 - BASE_HALF,
                        );
                        crate::web::set_animated_f32(
                            node,
                            AnimProp::TranslateY,
                            p.position.1 - BASE_HALF,
                        );
                        crate::web::set_animated_f32(node, AnimProp::Scale, scale);
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let _ = handle;
                }
            });
        }
        for w in &self.walls {
            let _ = w.view_ref.with(|handle| {
                #[cfg(target_arch = "wasm32")]
                {
                    if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                        if !w.active {
                            crate::web::set_animated_f32(node, AnimProp::Scale, 0.0);
                            return;
                        }
                        let (rx, ry, rw, rh) = w.rect;
                        // Same maths as for particles but with
                        // independent X/Y scales so the rect's
                        // aspect ratio is preserved.
                        crate::web::set_animated_f32(
                            node,
                            AnimProp::TranslateX,
                            rx + rw / 2.0 - BASE_HALF,
                        );
                        crate::web::set_animated_f32(
                            node,
                            AnimProp::TranslateY,
                            ry + rh / 2.0 - BASE_HALF,
                        );
                        crate::web::set_animated_f32(
                            node,
                            AnimProp::ScaleX,
                            rw / BASE_SIZE_PX,
                        );
                        crate::web::set_animated_f32(
                            node,
                            AnimProp::ScaleY,
                            rh / BASE_SIZE_PX,
                        );
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    let _ = handle;
                }
            });
        }
    }
}

/// Minimum side length for a finished wall. Tiny accidental drags
/// (a tap with 2 px of jitter) shouldn't litter the canvas with
/// pixel-sized blockers — anything smaller than this gets dropped
/// on touch-end.
const WALL_MIN_SIDE_PX: f32 = 6.0;

/// What the canvas is currently doing with the active gesture.
/// Switched on each touch `Began`; references the slot the gesture
/// is mutating so subsequent `Moved` events can resize that wall /
/// re-aim that particle without having to scan.
#[derive(Clone, Copy)]
enum GestureState {
    Idle,
    /// Currently drawing a wall — `slot` is the index in
    /// `sim.walls`. `start` is the Began pointer position.
    DrawingWall {
        slot: usize,
        start: (f32, f32),
    },
    /// Currently aiming a particle. A preview particle is mounted
    /// at the cursor position from Began onward; the cursor's
    /// motion drives both the particle's position and an
    /// EMA-smoothed velocity. On Ended we copy that velocity onto
    /// the particle and clear the preview flag so it joins the
    /// sim with the gesture's measured throw.
    AddingParticle {
        slot: usize,
        last_pos: (f32, f32),
        last_ts_ns: u64,
        /// Smoothed velocity in px/s. EMA-mixed on every Moved.
        velocity: (f32, f32),
    },
}

/// EMA mix factor for the add-particle velocity tracker. Higher =
/// more weight on the latest sample; lower = smoother but laggier.
/// `0.6` matches the pan recognizer's smoothing constant.
const ADD_PARTICLE_VELOCITY_SMOOTHING: f32 = 0.6;

fn particle_sim_section() -> Primitive {
    let sim = Rc::new(RefCell::new(ParticleSim::new(SIM_WIDTH, SIM_HEIGHT)));

    // Seed the simulation. Slot 0 is the big ball — we need a
    // stable index for the throw handler, so it goes first and
    // never moves. Slots 1..=N_SMALL are mints seeded on a ring
    // with a tangential velocity. The remainder of the pool stays
    // inactive until the user spawns from the toolbar.
    let cx = SIM_WIDTH / 2.0;
    let cy = SIM_HEIGHT / 2.0;
    let ring_r = (SIM_WIDTH.min(SIM_HEIGHT) / 2.0) - R_SMALL - 14.0;
    let init_speed = 70.0;
    let ball_index: usize = 0;
    {
        let mut s = sim.borrow_mut();
        s.ball_index = ball_index;
        s.push(Particle {
            position: (cx, cy),
            velocity: (0.0, 0.0),
            mass: M_BIG,
            radius: R_BIG,
            active: true,
            preview: false,
            is_ball: true,
            view_ref: Ref::new(),
        });
        for i in 0..N_SMALL {
            let theta = (i as f32) * std::f32::consts::TAU / (N_SMALL as f32);
            s.push(Particle {
                position: (cx + ring_r * theta.cos(), cy + ring_r * theta.sin()),
                velocity: (-theta.sin() * init_speed, theta.cos() * init_speed),
                mass: M_SMALL,
                radius: R_SMALL,
                active: true,
                preview: false,
                is_ball: false,
                view_ref: Ref::new(),
            });
        }
        // Inactive pool — pre-mounted so the user can spawn
        // particles without needing reactive children. Each slot
        // is sized like a small mint; the renderer hides them at
        // scale 0 until activated.
        while s.particles.len() < MAX_PARTICLES {
            s.push(Particle {
                position: (-1000.0, -1000.0),
                velocity: (0.0, 0.0),
                mass: M_SMALL,
                radius: R_SMALL,
                active: false,
                preview: false,
                is_ball: false,
                view_ref: Ref::new(),
            });
        }
        // Wall pool — same shape.
        for _ in 0..MAX_WALLS {
            s.walls.push(Wall {
                rect: (0.0, 0.0, 0.0, 0.0),
                active: false,
                view_ref: Ref::new(),
            });
        }
    }

    // Toolbar state.
    let mode: Signal<Mode> = signal!(Mode::Throw);
    let restitution: Signal<f32> = signal!(0.96);
    let air_drag: Signal<f32> = signal!(0.0);

    // Wire slider values into the sim's runtime parameters via an
    // Effect — reads both signals and copies them into the sim on
    // every change. Cheap: the Effect only fires when a slider
    // moves, not per frame.
    {
        let sim = sim.clone();
        let _e = Effect::new(move || {
            let r = restitution.get();
            let d = air_drag.get();
            let mut s = sim.borrow_mut();
            s.restitution = r;
            s.drag = d;
        });
        let _ = Box::leak(Box::new(_e));
    }

    // Canvas ref filled at mount. The tick reads `frame()` each
    // frame to keep the physics walls aligned with the rendered
    // size, so a window resize naturally re-anchors the bounds.
    let canvas_ref: Ref<ViewHandle> = Ref::new();

    {
        let sim = sim.clone();
        let canvas_ref = canvas_ref;
        let tick = framework_core::animation::clock::register_guarded(Box::new(
            move |dt| {
                if let Some(Some(rect)) = canvas_ref.with(|h| h.frame()) {
                    if rect.width > 1.0 && rect.height > 1.0 {
                        let mut s = sim.borrow_mut();
                        s.width = rect.width;
                        s.height = rect.height;
                    }
                }
                sim.borrow_mut().step(dt);
                true
            },
        ));
        let _ = Box::leak(Box::new(tick));
    }

    // Build the views. Walls render first so particles sit on top
    // — useful when a wall and a particle's centre overlap during
    // a draw-in-progress gesture.
    let (particle_views, wall_views) = {
        let s = sim.borrow();
        let particles: Vec<Primitive> = s
            .particles
            .iter()
            .map(|p| {
                let bg = if p.is_ball { "#7a3aff" } else { "#1abc9c" };
                view(vec![])
                    .with_style(pool_circle_sheet(bg))
                    .bind(p.view_ref)
                    .into()
            })
            .collect();
        let walls: Vec<Primitive> = s
            .walls
            .iter()
            .map(|w| {
                view(vec![])
                    .with_style(pool_rect_sheet())
                    .bind(w.view_ref)
                    .into()
            })
            .collect();
        (particles, walls)
    };
    let mut canvas_children: Vec<Primitive> = Vec::new();
    canvas_children.extend(wall_views);
    canvas_children.extend(particle_views);

    // The Throw-mode handler still uses the pan recognizer so we
    // get its smoothed velocity estimate on release. DrawWall and
    // AddParticle modes need their own raw-touch state machines
    // because they care about the initial down position (which
    // the pan recognizer hides behind its `slop_px` filter) and,
    // for AddParticle, want pure-tap (no-drag) to spawn at rest.
    let drag_origin = Rc::new(std::cell::Cell::new((0.0_f32, 0.0_f32)));
    let throw_handler = pan(PanRecognizer::new(), {
        let sim = sim.clone();
        let drag_origin = drag_origin.clone();
        move |ev| match ev {
            PanEvent::Began { .. } => {
                let mut s = sim.borrow_mut();
                let ball = &mut s.particles[ball_index];
                ball.velocity = (0.0, 0.0);
                drag_origin.set(ball.position);
            }
            PanEvent::Moved { delta, .. } => {
                let mut s = sim.borrow_mut();
                let width = s.width;
                let height = s.height;
                let ball = &mut s.particles[ball_index];
                let (ox, oy) = drag_origin.get();
                let r = ball.radius;
                ball.position = (
                    (ox + delta.x).clamp(r, width - r),
                    (oy + delta.y).clamp(r, height - r),
                );
                ball.velocity = (0.0, 0.0);
            }
            PanEvent::Ended { velocity } => {
                let mut s = sim.borrow_mut();
                s.particles[ball_index].velocity = (velocity.x, velocity.y);
            }
            PanEvent::Cancelled => {
                let mut s = sim.borrow_mut();
                s.particles[ball_index].velocity = (0.0, 0.0);
            }
        }
    });

    let gesture_state = Rc::new(std::cell::Cell::new(GestureState::Idle));
    let canvas_handler = {
        let sim = sim.clone();
        let throw_handler = throw_handler;
        let gesture_state = gesture_state.clone();
        move |ev: &TouchEvent| -> TouchResponse {
            match mode.get() {
                Mode::Throw => throw_handler(ev),
                Mode::DrawWall => handle_draw_wall(ev, &sim, &gesture_state),
                Mode::AddParticle => handle_add_particle(ev, &sim, &gesture_state),
            }
        }
    };

    let canvas: Primitive = view(canvas_children)
        .with_style(canvas_sheet())
        .on_touch(canvas_handler)
        .bind(canvas_ref)
        .into();

    let toolbar = toolbar_section(mode, restitution, air_drag);

    view(vec![toolbar, canvas])
        .with_style(section_sheet())
        .into()
}

/// `Mode::DrawWall` touch dispatch. Tracks a single wall slot
/// across a gesture's lifetime; resizes the slot to span from
/// the Began position to the current Moved position. Releases
/// smaller than `WALL_MIN_SIDE_PX` are discarded (accidental
/// taps shouldn't litter the field).
fn handle_draw_wall(
    ev: &TouchEvent,
    sim: &Rc<RefCell<ParticleSim>>,
    state: &Rc<std::cell::Cell<GestureState>>,
) -> TouchResponse {
    match ev.phase {
        TouchPhase::Began => {
            let pos = (ev.position.x, ev.position.y);
            let slot = sim
                .borrow_mut()
                .put_wall(None, (pos.0, pos.1, 0.0, 0.0));
            if let Some(slot) = slot {
                state.set(GestureState::DrawingWall { slot, start: pos });
            }
            TouchResponse::CONSUMED
        }
        TouchPhase::Moved => {
            if let GestureState::DrawingWall { slot, start } = state.get() {
                let cx = ev.position.x;
                let cy = ev.position.y;
                let rect = rect_between(start, (cx, cy));
                let _ = sim.borrow_mut().put_wall(Some(slot), rect);
            }
            TouchResponse::CONSUMED
        }
        TouchPhase::Ended => {
            if let GestureState::DrawingWall { slot, start } = state.get() {
                let rect = rect_between(start, (ev.position.x, ev.position.y));
                let mut s = sim.borrow_mut();
                if rect.2 < WALL_MIN_SIDE_PX || rect.3 < WALL_MIN_SIDE_PX {
                    // Reject — too small. Mark inactive.
                    s.walls[slot].active = false;
                } else {
                    let _ = s.put_wall(Some(slot), rect);
                }
            }
            state.set(GestureState::Idle);
            TouchResponse::CONSUMED
        }
        TouchPhase::Cancelled => {
            if let GestureState::DrawingWall { slot, .. } = state.get() {
                sim.borrow_mut().walls[slot].active = false;
            }
            state.set(GestureState::Idle);
            TouchResponse::CONSUMED
        }
    }
}

/// `Mode::AddParticle` touch dispatch.
///
/// - **Began**: spawn a *preview* particle at the cursor — visible
///   right away but excluded from physics, so it sits where the
///   user's finger is without bouncing off anything.
/// - **Moved**: re-write the preview's position to the new cursor
///   point and EMA-smooth a velocity estimate from
///   `Δposition / Δt`. The smoothing matches the pan recognizer's
///   so a release after a brief pause spawns gently rather than
///   teleporting at the last micro-flick's speed.
/// - **Ended**: copy the smoothed velocity onto the preview slot
///   and clear the preview flag — now it's a regular particle the
///   simulation steps every frame.
/// - **Cancelled**: drop the preview slot back to inactive.
fn handle_add_particle(
    ev: &TouchEvent,
    sim: &Rc<RefCell<ParticleSim>>,
    state: &Rc<std::cell::Cell<GestureState>>,
) -> TouchResponse {
    match ev.phase {
        TouchPhase::Began => {
            let pos = (ev.position.x, ev.position.y);
            let mut s = sim.borrow_mut();
            if let Some(slot) = s.spawn_preview_particle(pos) {
                state.set(GestureState::AddingParticle {
                    slot,
                    last_pos: pos,
                    last_ts_ns: ev.timestamp_ns,
                    velocity: (0.0, 0.0),
                });
            }
            TouchResponse::CONSUMED
        }
        TouchPhase::Moved => {
            if let GestureState::AddingParticle {
                slot,
                last_pos,
                last_ts_ns,
                velocity,
            } = state.get()
            {
                let now = (ev.position.x, ev.position.y);
                let dt_ns = ev.timestamp_ns.saturating_sub(last_ts_ns);
                let dt = (dt_ns as f32) * 1e-9;
                let new_velocity = if dt > 0.0 {
                    let raw = (
                        (now.0 - last_pos.0) / dt,
                        (now.1 - last_pos.1) / dt,
                    );
                    let a = ADD_PARTICLE_VELOCITY_SMOOTHING;
                    (
                        velocity.0 * (1.0 - a) + raw.0 * a,
                        velocity.1 * (1.0 - a) + raw.1 * a,
                    )
                } else {
                    velocity
                };
                sim.borrow_mut().set_preview_position(slot, now);
                state.set(GestureState::AddingParticle {
                    slot,
                    last_pos: now,
                    last_ts_ns: ev.timestamp_ns,
                    velocity: new_velocity,
                });
            }
            TouchResponse::CONSUMED
        }
        TouchPhase::Ended => {
            if let GestureState::AddingParticle { slot, velocity, .. } = state.get() {
                sim.borrow_mut().commit_preview(slot, velocity);
            }
            state.set(GestureState::Idle);
            TouchResponse::CONSUMED
        }
        TouchPhase::Cancelled => {
            if let GestureState::AddingParticle { slot, .. } = state.get() {
                sim.borrow_mut().cancel_preview(slot);
            }
            state.set(GestureState::Idle);
            TouchResponse::CONSUMED
        }
    }
}

/// Top-left + size rect spanning the two corner points, with
/// `width` / `height` always non-negative (user can drag in any
/// direction).
fn rect_between(a: (f32, f32), b: (f32, f32)) -> (f32, f32, f32, f32) {
    let x = a.0.min(b.0);
    let y = a.1.min(b.1);
    let w = (a.0 - b.0).abs();
    let h = (a.1 - b.1).abs();
    (x, y, w, h)
}

// =============================================================================
// Toolbar — mode buttons + friction sliders
// =============================================================================

fn toolbar_section(
    mode: Signal<Mode>,
    restitution: Signal<f32>,
    air_drag: Signal<f32>,
) -> Primitive {
    let mode_row = view(vec![
        mode_button("Throw", Mode::Throw, mode).into(),
        mode_button("Draw wall", Mode::DrawWall, mode).into(),
        mode_button("Add particle", Mode::AddParticle, mode).into(),
    ])
    .with_style(toolbar_row_sheet());

    let bounce_slider = labeled_slider(
        "Bounce",
        restitution,
        0.5,
        1.0,
        move |v| restitution.set(v),
    );
    let drag_slider =
        labeled_slider("Drag", air_drag, 0.0, 4.0, move |v| air_drag.set(v));

    let sliders_row = view(vec![bounce_slider, drag_slider])
        .with_style(toolbar_row_sheet());

    view(vec![mode_row.into(), sliders_row.into()])
        .with_style(toolbar_col_sheet())
        .into()
}

fn mode_button(label: &'static str, target: Mode, mode: Signal<Mode>) -> Primitive {
    let tap_handler = tap(TapRecognizer::new(), move || {
        mode.set(target);
    });

    view(vec![text(label).with_style(mode_button_label_sheet()).into()])
        .with_style(mode_button_sheet(target, mode))
        .on_touch(move |ev| tap_handler(ev))
        .into()
}

fn labeled_slider(
    label: &'static str,
    value: Signal<f32>,
    min: f32,
    max: f32,
    on_change: impl Fn(f32) + 'static,
) -> Primitive {
    view(vec![
        text(label).with_style(slider_label_sheet()).into(),
        slider(value, on_change).range(min, max).into(),
        // A live readout of the current value — reactive style on
        // a tiny `Text` node, refreshed by the framework on every
        // slider commit.
        text(move || format!("{:.2}", value.get()))
            .with_style(slider_value_sheet())
            .into(),
    ])
    .with_style(slider_group_sheet())
    .into()
}

/// Canvas — fills the full width of its column and grows
/// vertically to consume whatever space the section gives it. The
/// physics walls aren't pinned to `SIM_WIDTH`/`SIM_HEIGHT` any more
/// — they're re-read from `ViewHandle::frame()` every tick (see
/// the tick closure in `particle_sim_section`). The pixel
/// constants now only seed the initial particle ring.
fn canvas_sheet() -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(col("#191c25")),
        width: Some(pct(100.0)),
        position: Some(Position::Relative),
        overflow: Some(Overflow::Hidden),
        flex_grow: Some(1.0.into()),
        // Floor so on first layout the canvas reserves *some*
        // height — the row's cross-axis stretch handles the rest.
        min_height: Some(px(SIM_HEIGHT)),
        ..Default::default()
    };
    radius(&mut rules, 10.0);
    static_sheet(rules)
}

/// 100×100 base circle for the particle pool. The renderer scales
/// each instance to its physics radius via `AnimProp::Scale` and
/// translates to its position via `AnimProp::TranslateX/Y` — so a
/// single static stylesheet covers every entry in the pool, and
/// inactive slots just write `scale: 0` to vanish.
fn pool_circle_sheet(bg_hex: &'static str) -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(col(bg_hex)),
        width: Some(px(BASE_SIZE_PX)),
        height: Some(px(BASE_SIZE_PX)),
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        ..Default::default()
    };
    radius(&mut rules, BASE_HALF);
    static_sheet(rules)
}

/// 100×100 base rectangle for the wall pool. Same scale-based
/// sizing strategy as [`pool_circle_sheet`], but with a small
/// corner radius (not a full circle) and a wall-flavoured grey.
fn pool_rect_sheet() -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(col("#7a8390")),
        width: Some(px(BASE_SIZE_PX)),
        height: Some(px(BASE_SIZE_PX)),
        position: Some(Position::Absolute),
        top: Some(px(0.0)),
        left: Some(px(0.0)),
        ..Default::default()
    };
    radius(&mut rules, 4.0);
    static_sheet(rules)
}

fn toolbar_col_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Stretch),
        gap: Some(px(8.0)),
        ..Default::default()
    })
}

fn toolbar_row_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(px(10.0)),
        ..Default::default()
    })
}

/// Reactive style on the mode button. Reading `mode.get()` inside
/// the closure subscribes the framework's Effect, so the button
/// re-styles whenever the active mode changes (i.e. another button
/// gets tapped).
fn mode_button_sheet(target: Mode, mode: Signal<Mode>) -> impl Fn() -> StyleApplication + 'static {
    move || {
        let active = mode.get() == target;
        let (bg, fg, border) = if active {
            ("#7a3aff", "#ffffff", "#9b6dff")
        } else {
            ("#2a2f3c", "#dde1ea", "#3d4356")
        };
        let mut rules = StyleRules {
            background: Some(col(bg)),
            color: Some(col(fg)),
            padding_top: Some(px(8.0)),
            padding_bottom: Some(px(8.0)),
            padding_left: Some(px(14.0)),
            padding_right: Some(px(14.0)),
            align_items: Some(AlignItems::Center),
            justify_content: Some(JustifyContent::Center),
            border_top_width: Some(Tokenized::Literal(1.0)),
            border_right_width: Some(Tokenized::Literal(1.0)),
            border_bottom_width: Some(Tokenized::Literal(1.0)),
            border_left_width: Some(Tokenized::Literal(1.0)),
            border_top_color: Some(col(border)),
            border_right_color: Some(col(border)),
            border_bottom_color: Some(col(border)),
            border_left_color: Some(col(border)),
            ..Default::default()
        };
        radius(&mut rules, 6.0);
        StyleApplication::new(Rc::new(StyleSheet::r#static(rules)))
    }
}

fn mode_button_label_sheet() -> Rc<StyleSheet> {
    // No `color` — the label inherits the parent button's
    // foreground via the natural CSS cascade. Setting an explicit
    // colour here would shadow the parent's active/inactive
    // colour switch and lock the label to a single shade.
    static_sheet(StyleRules {
        font_size: Some(px(13.0)),
        ..Default::default()
    })
}

fn slider_group_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        align_items: Some(AlignItems::Center),
        gap: Some(px(8.0)),
        flex_grow: Some(1.0.into()),
        ..Default::default()
    })
}

fn slider_label_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(12.0)),
        color: Some(col("#8b93a3")),
        ..Default::default()
    })
}

fn slider_value_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(12.0)),
        color: Some(col("#f5f7fa")),
        ..Default::default()
    })
}

/// Section wrapping the caption + canvas. `flex_grow: 1` so it
/// claims the remaining horizontal space in the row;
/// `align_self: Stretch` lets it match the row's stretched height
/// instead of shrinking to its caption's intrinsic size.
fn section_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Stretch),
        gap: Some(px(10.0)),
        padding_top: Some(px(8.0)),
        flex_grow: Some(1.0.into()),
        align_self: Some(framework_core::AlignSelf::Stretch),
        ..Default::default()
    })
}

// =============================================================================
// AnimatedValue ↔ Signal bridge
// =============================================================================

/// Wire an animated scalar to a view's `AnimProp::Scale` via direct
/// inline-style writes through the backend's `set_animated_f32`.
///
/// This is the *fast path* — bypasses the reactive-style class
/// minting that's adequate for static/transition rules but adds
/// significant per-frame work when used for animation. Each tick
/// of `av` calls into the backend exactly once per node per frame,
/// which on web becomes one `element.style.setProperty(...)` call.
///
/// The subscription is leaked — this is a demo, and the page
/// lifetime is the test lifetime. A real app would tie it to a
/// scope.
fn drive_scale(av: &AnimatedValue<f32>, view_ref: Ref<ViewHandle>) {
    drive_via_ref(av, view_ref, AnimProp::Scale);
}

/// Counterpart for translate-X. Same pattern, different prop.
fn drive_translate_x(av: &AnimatedValue<f32>, view_ref: Ref<ViewHandle>) {
    drive_via_ref(av, view_ref, AnimProp::TranslateX);
}

/// Subscribe `av` so every per-frame value is written to the
/// node referenced by `view_ref` under `prop`. Until the ref is
/// filled (the walker hasn't mounted the view yet) the listener
/// silently skips. After mount, every frame writes one inline
/// CSS property.
fn drive_via_ref(av: &AnimatedValue<f32>, view_ref: Ref<ViewHandle>, prop: AnimProp) {
    let sub = av.subscribe_and_apply(move |v, _vel| {
        let value = *v;
        view_ref.with(|handle| {
            // Native targets receive the same listener but lack the
            // web-side helper; the `cfg` block ensures the example
            // crate builds for ios / android while only the web
            // path performs the actual property write.
            #[cfg(target_arch = "wasm32")]
            {
                if let Some(node) = handle.as_any().downcast_ref::<web_sys::Node>() {
                    crate::web::set_animated_f32(node, prop, value);
                }
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                let _ = (handle, value, prop);
            }
        });
    });
    Box::leak(Box::new(sub));
}

/// Per-platform debug log. Mirrors `touch-test`'s `log_line` minus
/// the on-screen overlay (browser DevTools console / Xcode console
/// / logcat are enough for a smoke test).
fn log(line: &str) {
    #[cfg(not(target_arch = "wasm32"))]
    println!("{line}");
    #[cfg(target_arch = "wasm32")]
    web_sys::console::log_1(&line.into());
}

// =============================================================================
// Stylesheets — kept flat and theme-less. The example exercises
// the animation system; styling stays out of the way.
// =============================================================================

fn pct(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Percent(v))
}

fn px(v: f32) -> Tokenized<Length> {
    Tokenized::Literal(Length::Px(v))
}

fn col(hex: &str) -> Tokenized<Color> {
    Tokenized::Literal(Color(hex.to_string()))
}

fn radius(rules: &mut StyleRules, r: f32) {
    rules.border_top_left_radius = Some(px(r));
    rules.border_top_right_radius = Some(px(r));
    rules.border_bottom_left_radius = Some(px(r));
    rules.border_bottom_right_radius = Some(px(r));
}

fn static_sheet(rules: StyleRules) -> Rc<StyleSheet> {
    Rc::new(StyleSheet::r#static(rules))
}

fn body_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Stretch),
        gap: Some(px(14.0)),
        padding_top: Some(px(24.0)),
        padding_right: Some(px(16.0)),
        padding_bottom: Some(px(40.0)),
        padding_left: Some(px(16.0)),
        background: Some(col("#0e1015")),
        // The mount point (`#app` in `index.html`) is sized to
        // `100%` of the viewport. Match that so the body fills the
        // page — without this the body would shrink to its
        // children's intrinsic height and the sim canvas would
        // never grow past the cards' column height.
        min_height: Some(pct(100.0)),
        ..Default::default()
    })
}

/// Row that holds the animated-cards column on the left and the
/// particle sim on the right. `align_items: FlexStart` so the
/// columns top-align — the cards column is taller than the sim, so
/// stretching would pull the sim's intrinsic size out of shape.
fn content_row_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        // Stretch — both columns end up the same height (the
        // taller one wins). That lets the sim canvas fill the
        // vertical space the animated cards' column takes.
        align_items: Some(AlignItems::Stretch),
        gap: Some(px(14.0)),
        // Consume the remaining vertical space in `body_sheet`
        // after the title, so the sim canvas can stretch all the
        // way to the bottom of the page rather than capping at the
        // cards-column height.
        flex_grow: Some(1.0.into()),
        ..Default::default()
    })
}

/// Left column of the body row: the animated cards stacked
/// vertically. `flex_grow: 1` so it takes whatever width's left
/// after the particle sim claims its intrinsic 320 px.
/// Left column of the body row: animated cards stacked vertically.
/// No `flex_grow` — the cards have an intrinsic width (`card_box_sheet`
/// pins to 280 px) so the column should hug its content and let the
/// particle sim claim the remaining horizontal space via its own
/// `flex_grow: 1`.
fn animated_col_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        align_items: Some(AlignItems::Stretch),
        gap: Some(px(14.0)),
        flex_shrink: Some(0.0.into()),
        ..Default::default()
    })
}

fn title_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        font_size: Some(px(24.0)),
        color: Some(col("#f5f7fa")),
        ..Default::default()
    })
}

fn card_caption_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        color: Some(col("#ffffff")),
        font_size: Some(px(13.0)),
        padding_top: Some(px(8.0)),
        padding_right: Some(px(10.0)),
        padding_bottom: Some(px(8.0)),
        padding_left: Some(px(10.0)),
        ..Default::default()
    })
}

fn button_label_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        color: Some(col("#0e1015")),
        font_size: Some(px(14.0)),
        ..Default::default()
    })
}

fn button_sheet() -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(col("#f5f7fa")),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        padding_top: Some(px(10.0)),
        padding_right: Some(px(14.0)),
        padding_bottom: Some(px(10.0)),
        padding_left: Some(px(14.0)),
        align_self: Some(framework_core::AlignSelf::FlexStart),
        ..Default::default()
    };
    radius(&mut rules, 8.0);
    static_sheet(rules)
}

fn stagger_section_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Column),
        gap: Some(px(10.0)),
        padding_top: Some(px(10.0)),
        padding_right: Some(px(10.0)),
        padding_bottom: Some(px(10.0)),
        padding_left: Some(px(10.0)),
        background: Some(col("#191c25")),
        ..Default::default()
    })
}

fn chip_row_sheet() -> Rc<StyleSheet> {
    static_sheet(StyleRules {
        flex_direction: Some(FlexDirection::Row),
        gap: Some(px(8.0)),
        ..Default::default()
    })
}

fn title_text(s: &'static str) -> framework_core::Bound<framework_core::TextHandle> {
    text(s).with_style(title_sheet())
}

/// Static card-box stylesheet. The transform is *not* declared here
/// — animated cards have their transform written inline by the
/// per-frame backend writer in [`drive_via_ref`]. Keeping the
/// background, layout, and radii static means `apply_style` runs
/// once at mount and never again, leaving all per-frame work to
/// the single `style.setProperty(...)` call the backend emits.
fn card_box_sheet(bg_hex: &'static str) -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(col(bg_hex)),
        width: Some(px(280.0)),
        height: Some(px(80.0)),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    };
    radius(&mut rules, 10.0);
    static_sheet(rules)
}

/// Smaller variant used by the stagger row's chips.
fn chip_box_sheet(bg_hex: &'static str) -> Rc<StyleSheet> {
    let mut rules = StyleRules {
        background: Some(col(bg_hex)),
        width: Some(px(40.0)),
        height: Some(px(40.0)),
        align_items: Some(AlignItems::Center),
        justify_content: Some(JustifyContent::Center),
        ..Default::default()
    };
    radius(&mut rules, 6.0);
    static_sheet(rules)
}
