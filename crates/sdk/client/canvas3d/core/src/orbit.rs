//! An orbit camera controller: drag to orbit around a target, pinch / wheel
//! to dolly, shift-drag or two fingers to pan.
//!
//! The camera state lives in a signal, so a painter that reads
//! [`OrbitCamera::camera`] re-renders as the user moves. Input arrives through
//! the ordinary `on_touch` / `on_wheel` props of the view wrapping the
//! `Canvas3d` — the controller hands out the handlers:
//!
//! ```ignore
//! ui! {
//!     view(on_touch = orbit.touch_handler(&view), on_wheel = orbit.wheel_handler()) {
//!         { Canvas3d(Canvas3dProps { draw: canvas3d::draw(move |s| { s.camera(orbit.camera()); /* … */ }), ..Default::default() }) }
//!     }
//! }
//! ```
//!
//! The math is in [`OrbitState`] (pure, value-in value-out) so it is tested
//! without input plumbing; the handlers only translate events into it.

use crate::camera::{Camera, Projection};
use crate::prim::Canvas3dHandle;
use glam::{Vec2, Vec3};
use runtime_core::{
    pointer_button, pointer_modifiers, signal, Signal, TouchEvent, TouchId, TouchPhase,
    TouchResponse, WheelEvent, WheelKind,
};
use std::cell::RefCell;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::rc::Rc;

/// Limits and feel of an [`OrbitCamera`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitConfig {
    /// Point orbited around.
    pub target: Vec3,
    /// Distance from the target.
    pub distance: f32,
    /// Angle around the vertical axis, radians (0 = looking along −Z from +Z).
    pub yaw: f32,
    /// Angle above the horizon, radians.
    pub pitch: f32,
    pub min_distance: f32,
    pub max_distance: f32,
    /// Pitch limits, radians. Kept strictly inside ±90° so the view never
    /// flips over the pole.
    pub min_pitch: f32,
    pub max_pitch: f32,
    /// Radians of orbit per logical pixel of drag.
    pub rotate_speed: f32,
    /// Dolly per wheel pixel: distance is multiplied by `exp(delta · speed)`.
    pub wheel_speed: f32,
    pub projection: Projection,
}

/// The pole margin pitch is kept away from (~1°).
const POLE_MARGIN: f32 = 0.0175;

impl Default for OrbitConfig {
    fn default() -> Self {
        OrbitConfig {
            target: Vec3::ZERO,
            distance: 5.0,
            yaw: 0.0,
            pitch: 0.35,
            min_distance: 0.1,
            max_distance: 1000.0,
            min_pitch: -FRAC_PI_2 + POLE_MARGIN,
            max_pitch: FRAC_PI_2 - POLE_MARGIN,
            rotate_speed: 0.008,
            wheel_speed: 0.002,
            projection: Projection::default(),
        }
    }
}

/// The controller's position: everything that changes as the user moves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OrbitState {
    pub target: Vec3,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
}

impl OrbitState {
    pub fn from_config(c: &OrbitConfig) -> OrbitState {
        OrbitState { target: c.target, distance: c.distance, yaw: c.yaw, pitch: c.pitch }
            .clamped(c)
    }

    fn clamped(mut self, c: &OrbitConfig) -> OrbitState {
        self.distance = self.distance.clamp(c.min_distance, c.max_distance);
        self.pitch = self.pitch.clamp(c.min_pitch, c.max_pitch);
        // Wrap yaw into (-π, π] so it never grows without bound.
        self.yaw = (self.yaw + PI).rem_euclid(TAU) - PI;
        self
    }

    /// Camera position.
    pub fn eye(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        self.target + Vec3::new(cp * sy, sp, cp * cy) * self.distance
    }

    pub fn camera(&self, c: &OrbitConfig) -> Camera {
        Camera { eye: self.eye(), target: self.target, up: Vec3::Y, projection: c.projection }
    }

    /// Orbit by a drag of `(dx, dy)` logical pixels: dragging right swings the
    /// camera left around the target (the content turns with the finger);
    /// dragging down raises the camera.
    pub fn orbit(self, dx: f32, dy: f32, c: &OrbitConfig) -> OrbitState {
        OrbitState {
            yaw: self.yaw - dx * c.rotate_speed,
            pitch: self.pitch + dy * c.rotate_speed,
            ..self
        }
        .clamped(c)
    }

    /// Multiply the distance by `factor` (> 1 = away).
    pub fn dolly(self, factor: f32, c: &OrbitConfig) -> OrbitState {
        if !factor.is_finite() || factor <= 0.0 {
            return self;
        }
        OrbitState { distance: self.distance * factor, ..self }.clamped(c)
    }

    /// Pan by a drag of `(dx, dy)` logical pixels in a view `view_height`
    /// logical pixels tall: the point under the finger at the target's depth
    /// stays under the finger.
    pub fn pan(self, dx: f32, dy: f32, view_height: f32, c: &OrbitConfig) -> OrbitState {
        if view_height <= 0.0 {
            return self;
        }
        let world_per_px = match c.projection {
            Projection::Perspective { fov_y, .. } => 2.0 * self.distance * (fov_y * 0.5).tan() / view_height,
            Projection::Orthographic { height, .. } => height / view_height,
        };
        let cam = self.camera(c);
        let forward = cam.forward();
        let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
        let up = right.cross(forward);
        OrbitState { target: self.target - right * dx * world_per_px + up * dy * world_per_px, ..self }
    }
}

/// Pointers currently down on the view (≤ 2 tracked).
#[derive(Default)]
struct Pointers {
    down: Vec<(TouchId, Vec2)>,
}

impl Pointers {
    fn centroid_and_spread(&self) -> Option<(Vec2, f32)> {
        match self.down.as_slice() {
            [(_, a), (_, b), ..] => Some(((*a + *b) * 0.5, a.distance(*b))),
            _ => None,
        }
    }
}

/// An orbit camera whose state is a signal. Create it in a component body
/// (the signal belongs to that scope).
#[derive(Clone)]
pub struct OrbitCamera {
    config: OrbitConfig,
    state: Signal<OrbitState>,
}

impl OrbitCamera {
    pub fn new(config: OrbitConfig) -> OrbitCamera {
        OrbitCamera { state: signal(OrbitState::from_config(&config)), config }
    }

    pub fn config(&self) -> &OrbitConfig {
        &self.config
    }

    /// The current camera — a reactive read.
    pub fn camera(&self) -> Camera {
        self.state.get().camera(&self.config)
    }

    /// The current state — a reactive read.
    pub fn state(&self) -> OrbitState {
        self.state.get()
    }

    pub fn set_state(&self, state: OrbitState) {
        self.state.set(state.clamped(&self.config));
    }

    /// Frame the camera on `target` at `distance`, keeping the angles.
    pub fn look_at(&self, target: Vec3, distance: f32) {
        let cfg = self.config;
        self.state.update(|s| OrbitState { target, distance, ..*s }.clamped(&cfg));
    }

    /// Handler for the wrapping view's `on_touch`: one pointer orbits
    /// (shift-held: pans), two pointers dolly by their spread and pan by
    /// their midpoint. `view` supplies the view's size for panning.
    ///
    /// The first pointer is consumed at `Began` (so the gesture keeps
    /// receiving motion even outside the view) and claimed once it moves,
    /// which stops an enclosing scroller from taking it. Non-primary mouse
    /// presses are ignored so they reach context-menu handlers.
    pub fn touch_handler(&self, view: &Canvas3dHandle) -> impl Fn(&TouchEvent) -> TouchResponse + 'static {
        let this = self.clone();
        let view = view.clone();
        let pointers = Rc::new(RefCell::new(Pointers::default()));
        move |ev: &TouchEvent| {
            let pos = Vec2::new(ev.position.x, ev.position.y);
            let mut p = pointers.borrow_mut();
            match ev.phase {
                TouchPhase::Began => {
                    if !pointer_button().is_primary() || p.down.len() >= 2 {
                        return TouchResponse::IGNORED;
                    }
                    p.down.push((ev.id, pos));
                    TouchResponse::CONSUMED
                }
                TouchPhase::Moved => {
                    let Some(i) = p.down.iter().position(|(id, _)| *id == ev.id) else {
                        return TouchResponse::IGNORED;
                    };
                    let before = p.centroid_and_spread();
                    let prev = std::mem::replace(&mut p.down[i].1, pos);
                    let after = p.centroid_and_spread();
                    let view_h = view.size().1;
                    let shift = pointer_modifiers().shift;
                    let cfg = &this.config;
                    // `update`, not `peek` + `set`: several motion events can
                    // arrive in one dispatch turn (Android batches them), and
                    // each must compose on the previous one's STAGED write —
                    // the committed value would drop every delta but the last.
                    this.state.update(|s| match (before, after) {
                        (Some((c0, d0)), Some((c1, d1))) => {
                            let s = if d1 > f32::EPSILON { s.dolly(d0 / d1, cfg) } else { *s };
                            let d = c1 - c0;
                            s.pan(d.x, d.y, view_h, cfg)
                        }
                        _ => {
                            let d = pos - prev;
                            if shift {
                                s.pan(d.x, d.y, view_h, cfg)
                            } else {
                                s.orbit(d.x, d.y, cfg)
                            }
                        }
                    });
                    TouchResponse::CLAIMED
                }
                TouchPhase::Ended | TouchPhase::Cancelled => {
                    p.down.retain(|(id, _)| *id != ev.id);
                    TouchResponse::CONSUMED
                }
                TouchPhase::Hovered => TouchResponse::IGNORED,
            }
        }
    }

    /// Handler for the wrapping view's `on_wheel`: scrolling or a trackpad
    /// pinch dollies. Consumed, so the page doesn't scroll or zoom instead.
    pub fn wheel_handler(&self) -> impl Fn(&WheelEvent) -> TouchResponse + 'static {
        let this = self.clone();
        move |ev: &WheelEvent| {
            let factor = match ev.kind {
                WheelKind::Scroll => (ev.delta_y * this.config.wheel_speed).exp(),
                // `scale > 1` = zoom in = move closer.
                WheelKind::Zoom => 1.0 / ev.scale,
                _ => return TouchResponse::IGNORED,
            };
            // Composes with other wheel events in the same turn (see the
            // touch handler).
            this.state.update(|s| s.dolly(factor, &this.config));
            TouchResponse::CONSUMED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> OrbitConfig {
        OrbitConfig::default()
    }

    #[test]
    fn eye_sits_at_distance_from_target() {
        let c = OrbitConfig { target: Vec3::new(1.0, 2.0, 3.0), distance: 4.0, ..cfg() };
        let s = OrbitState::from_config(&c);
        assert!(((s.eye() - c.target).length() - 4.0).abs() < 1e-5);
    }

    #[test]
    fn yaw_zero_pitch_zero_looks_down_negative_z() {
        let c = OrbitConfig { pitch: 0.0, ..cfg() };
        let cam = OrbitState::from_config(&c).camera(&c);
        assert!((cam.forward() - Vec3::NEG_Z).length() < 1e-5);
    }

    #[test]
    fn dragging_right_swings_the_camera_left() {
        let c = OrbitConfig { pitch: 0.0, ..cfg() };
        let s = OrbitState::from_config(&c).orbit(50.0, 0.0, &c);
        assert!(s.eye().x < 0.0, "camera moved to -X: {}", s.eye());
    }

    #[test]
    fn dragging_down_raises_the_camera() {
        let c = cfg();
        let s0 = OrbitState::from_config(&c);
        let s1 = s0.orbit(0.0, 40.0, &c);
        assert!(s1.eye().y > s0.eye().y);
    }

    #[test]
    fn pitch_is_clamped_short_of_the_poles() {
        let c = cfg();
        let s = OrbitState::from_config(&c).orbit(0.0, 1e6, &c);
        assert_eq!(s.pitch, c.max_pitch);
        let s = OrbitState::from_config(&c).orbit(0.0, -1e6, &c);
        assert_eq!(s.pitch, c.min_pitch);
    }

    #[test]
    fn yaw_wraps_instead_of_growing() {
        let c = cfg();
        let mut s = OrbitState::from_config(&c);
        for _ in 0..1000 {
            s = s.orbit(100.0, 0.0, &c);
        }
        assert!(s.yaw > -PI - 1e-4 && s.yaw <= PI + 1e-4, "{}", s.yaw);
    }

    #[test]
    fn dolly_clamps_and_rejects_nonsense() {
        let c = OrbitConfig { min_distance: 1.0, max_distance: 10.0, ..cfg() };
        let s = OrbitState::from_config(&c);
        assert_eq!(s.dolly(100.0, &c).distance, 10.0);
        assert_eq!(s.dolly(0.0001, &c).distance, 1.0);
        assert_eq!(s.dolly(f32::NAN, &c), s);
        assert_eq!(s.dolly(-2.0, &c), s);
    }

    /// Panning keeps the point under the finger fixed: after panning by
    /// (dx, dy), the old target projects `(dx, dy)` away from the centre.
    #[test]
    fn pan_moves_content_with_the_finger() {
        let c = cfg();
        let size = (800.0, 600.0);
        let s0 = OrbitState::from_config(&c);
        let s1 = s0.pan(40.0, -25.0, size.1, &c);
        let p = s1.camera(&c).project(s0.target, size).unwrap();
        assert!((p - Vec2::new(440.0, 275.0)).length() < 0.5, "{p}");
    }

    #[test]
    fn pan_without_a_view_height_does_nothing() {
        let c = cfg();
        let s = OrbitState::from_config(&c);
        assert_eq!(s.pan(10.0, 10.0, 0.0, &c), s);
    }
}
