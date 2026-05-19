//! Platform-agnostic interaction host.
//!
//! Owns the live `WgpuBackend`, the shared text + font-system
//! stores, and the per-interaction state (focused input, slider
//! drag, last pointer position). Exposes a small method surface
//! that platform shells (winit on desktop, browser, UIKit, …)
//! drive from their native event stream.
//!
//! Nothing in this module depends on a platform. The winit shim in
//! [`crate::app`] is one client; a future web or iOS shell would
//! consume the same `Host` API by translating its own events to
//! [`crate::input::PointerEvent`] / [`crate::input::KeyEvent`].
//!
//! # Threading
//!
//! Single-threaded. Same as the framework itself — `Signal`s,
//! `Effect`s, and the `Rc`-based node tree all live on the host
//! thread. Cross-thread input (background networking, audio) would
//! need to post into this thread before calling `pointer_*` /
//! `key`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use framework_core::{render, ColorScheme, Easing, Owner, Primitive, StateBits};
use glyphon::FontSystem;
use native_layout::LayoutNode;

use render_api::EventSink;
use crate::backend_impl::WgpuBackend;
use render_api::{Key, KeyEvent, PointerButton, PointerEvent, ScrollEvent};
use crate::keyboard;
use crate::node::{
    NodeKind, WgpuNode, HIT_SLOP, IOS_MIN_HIT_TARGET, KEYBOARD_ANIM_MS, KEYBOARD_HEIGHT,
    KEYBOARD_INPUT_MARGIN, PAN_THRESHOLD, SCROLL_MOMENTUM_DECAY_PER_SEC,
    SCROLL_MOMENTUM_MIN_VELOCITY, SCROLL_MOMENTUM_STALE_MS, SCROLL_RUBBERBAND_RESISTANCE,
    SCROLL_SPRINGBACK_EPSILON, SCROLL_SPRINGBACK_RATE_PER_SEC, SCROLL_VELOCITY_SMOOTHING,
    SLIDER_THUMB_SIZE,
};
use crate::skin::Skin;
use crate::text::TextStore;

pub struct Host {
    backend: Rc<RefCell<WgpuBackend>>,
    text: Rc<RefCell<TextStore>>,
    font_system: Rc<RefCell<FontSystem>>,
    /// Active platform skin. Plumbed through to the renderer for
    /// widget + keyboard paint, and to the keyboard hit-test path
    /// so taps on synthesized keys land in the right `KeySpec`s.
    skin: Rc<dyn Skin>,
    /// Currently keyboard-focused TextInput, if any. Cleared on
    /// click outside any input or on Esc.
    focused_input: Option<WgpuNode>,
    /// Active pointer-down interaction, if any. iOS-style: a press
    /// captures a node and a release action; whether the action
    /// fires depends on whether the pointer is still inside the
    /// node when the pointer comes up. Cleared on pointer-up /
    /// pointer-cancel.
    active_press: Option<ActivePress>,
    /// Live post-pan scroll motion (momentum-coast or
    /// rubber-band spring-back). Ticked by
    /// [`Host::tick_animations`] every frame until it settles
    /// (velocity below the threshold for Coast, distance below
    /// epsilon for SpringBack). A fresh pointer-down clears it
    /// (tap-to-catch).
    momentum: Option<ScrollMotion>,
    /// Most recent pointer position in logical px. Updated by
    /// every pointer event so slider drag has fresh coordinates
    /// when the platform only delivers a `down` (no `move`).
    pointer: (f32, f32),
    /// Logical viewport size in CSS pixels. Set by the shell via
    /// [`Host::set_viewport`] on startup and on resize; used to
    /// position the on-screen keyboard against the bottom edge.
    pub(crate) viewport: (f32, f32),
    /// Pre-built glyphon buffers for every on-screen keyboard
    /// label. Constructed once in [`Host::new`] so the renderer's
    /// read-only walk can borrow them straight out — no
    /// per-frame text allocation.
    pub(crate) keyboard_glyphs: HashMap<&'static str, glyphon::Buffer>,
    /// Current rest visibility of the on-screen keyboard:
    /// `0.0` = fully hidden, `1.0` = fully visible. Updated by
    /// [`Host::sync_keyboard`] whenever focus changes; the
    /// transition itself is interpolated by `keyboard_anim`.
    keyboard_value: f32,
    /// In-flight slide animation, if the keyboard is currently
    /// moving between hidden and visible. Cleared automatically
    /// when [`tick_animations`] notices the duration has elapsed.
    keyboard_anim: Option<KeyboardAnim>,
    /// Framework reactive scopes. Held so they outlive the host;
    /// cleared on `unmount` or drop.
    _owner: Option<Owner>,
}

#[derive(Copy, Clone, Debug)]
struct KeyboardAnim {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
    easing: Easing,
}

/// In-flight scroll-motion driving a scrollview's offset over
/// time. Two flavors:
/// - `Coast` — exponential-decay momentum after a fling. Drives
///   `offset += -velocity * dt`, decays `velocity *= exp(-k*dt)`.
/// - `SpringBack` — exponential approach to a target offset
///   after a rubber-band overshoot. Drives
///   `offset += (target - offset) * (1 - exp(-k*dt))`.
struct ScrollMotion {
    scrollview: WgpuNode,
    kind: ScrollMotionKind,
    last_tick: Instant,
}

enum ScrollMotionKind {
    Coast { velocity: (f32, f32) },
    SpringBack { target: (f32, f32) },
}

/// What a pointer-down created. Lives only while the pointer is
/// held; cleared on up/cancel.
enum ActivePress {
    /// Pressable / Button / Toggle: press → maybe-fire on release.
    /// If `scrollview` is set, dragging past [`PAN_THRESHOLD`]
    /// converts this into a `Pan` (cancelling the click).
    Click {
        node: WgpuNode,
        action: ReleaseAction,
        /// `true` while the pointer is over the captured node.
        /// Toggles as the pointer moves in/out of the bounds; the
        /// release action fires only if `over` is true at up.
        over: bool,
        /// Innermost scrollview at the press origin, if any.
        /// Lets a deliberate drag upgrade the press into a pan.
        scrollview: Option<WgpuNode>,
        /// Pointer position at press-down. Used to measure the
        /// drag distance against [`PAN_THRESHOLD`].
        start: (f32, f32),
    },
    /// Phone-style pan scrolling: pointer-drag inside (or starting
    /// from empty space within) a scrollview translates the
    /// content with the cursor. `last` is updated on every move so
    /// the next delta is relative; `last_time` is the timestamp of
    /// that update, used to derive the smoothed `velocity` (px/sec
    /// in screen space) that we hand to the momentum kick-off on
    /// release. `start` is the pointer position at the moment the
    /// pan began (either an empty-area press or the upgrade point
    /// from a `Click`) — `pointer_up` measures total motion
    /// against it to decide whether the gesture was a real pan
    /// or a tap on empty space.
    Pan {
        scrollview: WgpuNode,
        last: (f32, f32),
        last_time: Instant,
        velocity: (f32, f32),
        start: (f32, f32),
    },
    /// Slider: press fires the initial value, every move updates,
    /// release just ends the drag.
    SliderDrag { node: WgpuNode },
}

enum ReleaseAction {
    /// Pressable / Button.
    Fire(Rc<dyn Fn()>),
    /// Toggle: read current value at fire time and pass !current to
    /// `on_change`. Reading at release rather than press keeps us
    /// honest if the signal flipped via another path during the
    /// press.
    FlipToggle(Rc<dyn Fn(bool)>),
}

impl Host {
    pub fn new(skin: Rc<dyn Skin>, color_scheme: ColorScheme) -> Self {
        let text = Rc::new(RefCell::new(TextStore::new()));
        let font_system = Rc::new(RefCell::new(FontSystem::new()));
        // Pre-build keyboard glyph buffers for the active skin
        // while we have exclusive access to the font system. Cheap
        // (~30 small Buffers); the alternative is mutating the
        // font system from the read-only render walk, which would
        // require a `RefCell` borrow dance on the hot path.
        let keyboard_glyphs = keyboard::build_glyph_cache(
            &mut font_system.borrow_mut(),
            skin.as_ref(),
        );
        let backend = Rc::new(RefCell::new(WgpuBackend::new(
            text.clone(),
            font_system.clone(),
            color_scheme,
        )));
        Self {
            backend,
            text,
            font_system,
            skin,
            focused_input: None,
            active_press: None,
            momentum: None,
            pointer: (0.0, 0.0),
            viewport: (0.0, 0.0),
            keyboard_glyphs,
            keyboard_value: 0.0,
            keyboard_anim: None,
            _owner: None,
        }
    }

    /// Tell the host the current logical viewport size. The
    /// shell calls this on window creation and on resize so the
    /// on-screen keyboard knows where the bottom of the screen
    /// is.
    pub fn set_viewport(&mut self, w: f32, h: f32) {
        self.viewport = (w, h);
    }

    /// Is the simulator's on-screen keyboard currently visible
    /// (or animating in)? `true` means the renderer should paint
    /// it. The actual position is interpolated via
    /// [`Host::sample_keyboard`]; this method is a coarse
    /// "should I paint anything at all" check.
    pub fn keyboard_visible(&self) -> bool {
        self.focused_input.is_some() || self.keyboard_anim.is_some()
    }

    /// Current keyboard slide value, in `[0.0, 1.0]`:
    /// - `0.0` = fully hidden (off-screen below the viewport)
    /// - `1.0` = fully visible (resting at the bottom)
    /// The renderer translates the keyboard's y-position by
    /// `(1.0 - value) * KEYBOARD_HEIGHT` so the slide is
    /// continuous across frames.
    pub fn sample_keyboard(&self, now: Instant) -> f32 {
        let Some(a) = &self.keyboard_anim else {
            return self.keyboard_value;
        };
        let dur = a.duration.as_secs_f32();
        if dur <= 0.0 {
            return a.to;
        }
        let elapsed = now.saturating_duration_since(a.started).as_secs_f32();
        if elapsed >= dur {
            return a.to;
        }
        let t = elapsed / dur;
        let eased = ease(a.easing, t);
        a.from + (a.to - a.from) * eased
    }

    /// Reconcile the keyboard's rest value with the current focus
    /// state. Call after any event that may have changed focus
    /// (pointer-down on a non-input, key Escape, …). If the rest
    /// value differs from the focus state, kick off a slide
    /// animation between them — and, on focus-in, push the
    /// scrollview up so the input sits above the keyboard's
    /// final rest position (iOS auto-scroll).
    fn sync_keyboard(&mut self) {
        let desired: f32 = if self.focused_input.is_some() { 1.0 } else { 0.0 };
        if (desired - self.keyboard_value).abs() < f32::EPSILON {
            return;
        }
        let now = Instant::now();
        let from = self.sample_keyboard(now);
        self.keyboard_value = desired;
        self.keyboard_anim = Some(KeyboardAnim {
            from,
            to: desired,
            started: now,
            duration: Duration::from_millis(KEYBOARD_ANIM_MS as u64),
            easing: Easing::EaseOut,
        });
        crate::scheduler::request_redraw();

        // On focus-in, scroll the input into view above the
        // keyboard's final position. iOS does this automatically
        // via `UIScrollView`'s content-inset; we emulate by
        // tweening the scrollview's offset.
        if desired > 0.5 {
            self.auto_scroll_focused_into_view(now);
        }
    }

    /// If the currently focused input sits below the keyboard's
    /// final top edge, scroll its enclosing `ScrollView` up by
    /// enough to put the input's bottom edge above the keyboard
    /// (plus a small margin). Re-uses the SpringBack scroll
    /// motion for a smooth eased transition.
    fn auto_scroll_focused_into_view(&mut self, now: Instant) {
        let Some(input) = self.focused_input.clone() else { return };
        let target_layout = input.borrow().layout;
        let (vw, vh) = self.viewport;
        if vw <= 0.0 || vh <= 0.0 {
            return;
        }
        let kb_top = vh - KEYBOARD_HEIGHT.min(vh);

        let backend = self.backend.borrow();
        let Some(root) = backend.roots.first() else { return };
        let root = root.clone();

        // Locate the input's absolute frame *and* its enclosing
        // scrollview (innermost) in a single pre-order walk.
        let Some((input_y, input_h, sv)) =
            find_input_in_scrollview(&backend, &root, target_layout, 0.0, 0.0, None)
        else {
            return;
        };
        let input_bottom = input_y + input_h;
        let want_visible_bottom = kb_top - KEYBOARD_INPUT_MARGIN;
        if input_bottom <= want_visible_bottom {
            // Already above the keyboard's projected position.
            return;
        }

        // Push the scrollview's offset_y down by the overlap so
        // the input rides just above the keyboard. Clamp to the
        // scrollview's max so we never request an impossible
        // offset.
        let frame = backend.layout.frame_of(sv.borrow().layout);
        let extent = scrollview_content_extent(&backend, &sv);
        let max_y = (extent.1 - frame.height).max(0.0);
        let (cur_x, cur_y) = match &sv.borrow().kind {
            NodeKind::ScrollView { offset_x, offset_y, .. } => (*offset_x, *offset_y),
            _ => return,
        };
        drop(backend);
        let delta = input_bottom - want_visible_bottom;
        let target_y = (cur_y + delta).clamp(0.0, max_y);
        if (target_y - cur_y).abs() < f32::EPSILON {
            return;
        }
        self.momentum = Some(ScrollMotion {
            scrollview: sv,
            kind: ScrollMotionKind::SpringBack {
                target: (cur_x, target_y),
            },
            last_tick: now,
        });
        crate::scheduler::request_redraw();
    }

    /// Build and mount the framework tree against the backend. The
    /// returned `Owner` is held internally so reactive scopes live
    /// until the host is dropped.
    pub fn mount<F>(&mut self, build_ui: F)
    where
        F: FnOnce() -> Primitive,
    {
        if self._owner.is_some() {
            return;
        }
        let tree = build_ui();
        self._owner = Some(render(self.backend.clone(), tree));
    }

    // ---------------- Read-only accessors used by the renderer ---

    pub fn backend(&self) -> &Rc<RefCell<WgpuBackend>> { &self.backend }
    pub fn text_store(&self) -> &Rc<RefCell<TextStore>> { &self.text }
    pub fn font_system(&self) -> &Rc<RefCell<FontSystem>> { &self.font_system }
    /// The active skin. Renderer borrows this every frame to
    /// paint widget chrome + the on-screen keyboard.
    pub fn skin(&self) -> &Rc<dyn Skin> { &self.skin }
    pub fn focused_input_layout(&self) -> Option<LayoutNode> {
        self.focused_input.as_ref().map(|n| n.borrow().layout)
    }

    /// Advance the per-frame state that the renderer samples:
    /// - the tween engine (toggle slide, theme crossfade, …)
    /// - momentum scroll (post-pan deceleration)
    /// - keyboard slide animation
    /// - caret blink (drives a continuous redraw while a
    ///   TextInput is focused so the blink stays in motion)
    ///
    /// Returns `true` if anything still needs animation — the
    /// shell's render loop should `request_redraw` so the next
    /// frame catches the next step.
    pub fn tick(&mut self, now: Instant) -> bool {
        let any_anim = self.backend.borrow_mut().animator.tick(now);
        let any_momentum = self.tick_momentum(now);
        // Tick the keyboard slide. Once duration elapses, drop
        // the anim so we stop firing redraws.
        let mut kb_alive = false;
        if let Some(a) = self.keyboard_anim {
            if now.saturating_duration_since(a.started) >= a.duration {
                self.keyboard_anim = None;
            } else {
                kb_alive = true;
            }
        }
        let caret_alive = self.focused_input.is_some();
        // Any visible spinner needs the next frame to advance its
        // rotation phase.
        let spinner_alive = self.backend.borrow().active_spinner_count > 0;
        any_anim || any_momentum || kb_alive || caret_alive || spinner_alive
    }

    /// Single tick of the active scroll motion. Handles both:
    /// - `Coast` — momentum-fling deceleration. Hard-clamps at
    ///   the bounds; on a boundary hit, zeros that axis's
    ///   velocity. If a clamp happened, fold a `SpringBack`
    ///   onto the very same scrollview's bound if rubber-band
    ///   overshoot is in play (typically none at coast — the
    ///   pan was the rubber-band path; coast hard-clamps).
    /// - `SpringBack` — exponential approach to the target
    ///   offset. Returns false when the gap drops below
    ///   `SCROLL_SPRINGBACK_EPSILON`.
    fn tick_momentum(&mut self, now: Instant) -> bool {
        let Some(mut m) = self.momentum.take() else { return false };
        // Cap `dt` so a paused tab or long blocking call doesn't
        // teleport the offset across the entire content.
        let dt = now.duration_since(m.last_tick).as_secs_f32().min(0.1);
        m.last_tick = now;

        match &mut m.kind {
            ScrollMotionKind::Coast { velocity } => {
                let before = match &m.scrollview.borrow().kind {
                    NodeKind::ScrollView { offset_x, offset_y, .. } => {
                        (*offset_x, *offset_y)
                    }
                    _ => return false,
                };
                self.apply_scroll(
                    &m.scrollview,
                    -velocity.0 * dt,
                    -velocity.1 * dt,
                    false,
                );
                let after = match &m.scrollview.borrow().kind {
                    NodeKind::ScrollView { offset_x, offset_y, .. } => {
                        (*offset_x, *offset_y)
                    }
                    _ => return false,
                };
                // Boundary hit → zero that axis. Without this
                // the coast keeps trying to push past a clamped
                // offset and only ends on the decay timer.
                if (after.0 - before.0).abs() < f32::EPSILON {
                    velocity.0 = 0.0;
                }
                if (after.1 - before.1).abs() < f32::EPSILON {
                    velocity.1 = 0.0;
                }
                let decay = (-SCROLL_MOMENTUM_DECAY_PER_SEC * dt).exp();
                velocity.0 *= decay;
                velocity.1 *= decay;
                let speed_sq = velocity.0 * velocity.0 + velocity.1 * velocity.1;
                let min_sq = SCROLL_MOMENTUM_MIN_VELOCITY * SCROLL_MOMENTUM_MIN_VELOCITY;
                if speed_sq < min_sq {
                    return false;
                }
            }
            ScrollMotionKind::SpringBack { target } => {
                // Exponential approach: position += (target -
                // position) * (1 - exp(-k*dt)). Framerate-stable
                // (same wall-clock rate at any tick frequency).
                let factor = 1.0 - (-SCROLL_SPRINGBACK_RATE_PER_SEC * dt).exp();
                let (cur_x, cur_y) = match &m.scrollview.borrow().kind {
                    NodeKind::ScrollView { offset_x, offset_y, .. } => {
                        (*offset_x, *offset_y)
                    }
                    _ => return false,
                };
                let dx = (target.0 - cur_x) * factor;
                let dy = (target.1 - cur_y) * factor;
                if let NodeKind::ScrollView {
                    offset_x, offset_y, ..
                } = &mut m.scrollview.borrow_mut().kind
                {
                    *offset_x = cur_x + dx;
                    *offset_y = cur_y + dy;
                }
                crate::scheduler::request_redraw();
                let gap = ((target.0 - cur_x - dx).powi(2)
                    + (target.1 - cur_y - dy).powi(2))
                .sqrt();
                if gap < SCROLL_SPRINGBACK_EPSILON {
                    // Snap to target so the final position is
                    // exactly the bound (no sub-pixel drift).
                    if let NodeKind::ScrollView {
                        offset_x, offset_y, ..
                    } = &mut m.scrollview.borrow_mut().kind
                    {
                        *offset_x = target.0;
                        *offset_y = target.1;
                    }
                    return false;
                }
            }
        }
        self.momentum = Some(m);
        true
    }

    // ---------------- Event entry points -------------------------

    /// Pointer moved. Drives:
    /// - Slider drag → value update.
    /// - Click press whose `scrollview` is set → if cumulative
    ///   drag exceeds [`PAN_THRESHOLD`], cancel the press (clear
    ///   PRESSED state) and convert to a `Pan`. Otherwise update
    ///   the `over` flag for visual press feedback.
    /// - Pan → translate the scrollview's content by the delta
    ///   from `last`.
    pub fn pointer_move(&mut self, ev: PointerEvent) {
        self.pointer = ev.position;
        let Some(press) = self.active_press.take() else { return };
        match press {
            ActivePress::SliderDrag { node } => {
                self.update_slider_drag(&node);
                self.active_press = Some(ActivePress::SliderDrag { node });
            }
            ActivePress::Pan {
                scrollview,
                last,
                last_time,
                velocity,
                start,
            } => {
                let dx = ev.position.0 - last.0;
                let dy = ev.position.1 - last.1;
                // Drag-and-stick semantics: a finger moving down
                // (positive dy) drags the content down. Content
                // moving down = scroll offset *decreases* (we're
                // revealing what's above). So we feed `-dy`.
                // `rubber_band: true` so the user can drag past
                // the edges with damped overshoot — released
                // past-bound, the spring-back kicks in.
                self.apply_scroll(&scrollview, -dx, -dy, true);

                // Update smoothed velocity (px/sec, screen-space).
                // Clamp dt to a tiny floor so two same-frame moves
                // don't produce explosive raw velocities.
                let now = Instant::now();
                let dt = now
                    .duration_since(last_time)
                    .as_secs_f32()
                    .max(0.001);
                let raw = (dx / dt, dy / dt);
                let a = SCROLL_VELOCITY_SMOOTHING;
                let new_velocity = (
                    velocity.0 * (1.0 - a) + raw.0 * a,
                    velocity.1 * (1.0 - a) + raw.1 * a,
                );

                self.active_press = Some(ActivePress::Pan {
                    scrollview,
                    last: ev.position,
                    last_time: now,
                    velocity: new_velocity,
                    start,
                });
            }
            ActivePress::Click {
                node,
                action,
                over,
                scrollview,
                start,
            } => {
                // If the press is over a scrollview and the
                // drag has gone past the slop, upgrade to a pan
                // and cancel the click. Preserve the original
                // press position as the pan's `start` so the
                // total-motion check at `pointer_up` reflects
                // the whole gesture, not just post-upgrade
                // motion.
                if let Some(sv) = scrollview.as_ref() {
                    let dx = ev.position.0 - start.0;
                    let dy = ev.position.1 - start.1;
                    if (dx * dx + dy * dy).sqrt() > PAN_THRESHOLD {
                        set_state(&node, StateBits::PRESSED, false);
                        self.active_press = Some(ActivePress::Pan {
                            scrollview: sv.clone(),
                            last: ev.position,
                            last_time: Instant::now(),
                            velocity: (0.0, 0.0),
                            start,
                        });
                        return;
                    }
                }
                // Within slop: keep tracking the press, just
                // update the over-flag for visual feedback.
                let new_over = self.pointer_over_node(&node, ev.position);
                if new_over != over {
                    set_state(&node, StateBits::PRESSED, new_over);
                }
                self.active_press = Some(ActivePress::Click {
                    node,
                    action,
                    over: new_over,
                    scrollview,
                    start,
                });
            }
        }
    }

    /// Pointer pressed. Picks an interaction based on what's
    /// under the pointer:
    /// - Pressable / Button → capture; fire on release-inside
    ///   (drag past `PAN_THRESHOLD` upgrades to scroll-pan if a
    ///   scrollview is under the press).
    /// - Toggle           → capture; flip on release-inside.
    /// - Slider           → capture; emit initial value, start drag.
    /// - TextInput        → focus immediately (iOS keyboard model).
    /// - Empty inside SV  → start pan immediately.
    /// - nothing          → drop any active TextInput focus.
    pub fn pointer_down(&mut self, ev: PointerEvent) {
        if !matches!(ev.button, PointerButton::Primary) {
            return;
        }
        self.pointer = ev.position;
        // Tap-to-catch: a press anywhere stops any in-flight
        // momentum scroll, matching `UIScrollView`'s behavior of
        // halting deceleration on touch-down.
        self.momentum = None;

        // On-screen keyboard intercept. While an input is
        // focused, the keyboard overlay is painted at the bottom
        // of the viewport; presses inside it never reach the app
        // tree. A tap on a key synthesizes a `KeyEvent` and
        // forwards it through the same handler that processes
        // physical keys — same code path, same behavior. Presses
        // in the keyboard gutter (gaps between keys) are
        // swallowed so the app underneath doesn't twitch.
        if self.keyboard_visible() {
            let slide = self.sample_keyboard(Instant::now());
            let (kb_rect, hit) =
                keyboard::hit_test(self.skin.as_ref(), self.viewport, slide, ev.position);
            if let Some(action) = hit {
                let ke = keyboard::action_to_key_event(action);
                self.key(&ke);
                return;
            }
            // Inside keyboard frame but no key → swallow.
            if let Some(rect) = kb_rect {
                let inside = ev.position.0 >= rect.0
                    && ev.position.0 <= rect.0 + rect.2
                    && ev.position.1 >= rect.1
                    && ev.position.1 <= rect.1 + rect.3;
                if inside {
                    return;
                }
            }
        }

        // Hit-test in one pass for both an interactive leaf and
        // the innermost scrollview ancestor at this point.
        let (hit, scrollview_at) = {
            let backend = self.backend.borrow();
            let Some(root) = backend.root() else { return };
            let h = hit_test_node(&backend, &root, 0.0, 0.0, ev.position);
            let sv = find_scroll_view_at(&backend, &root, 0.0, 0.0, ev.position);
            (h, sv)
        };

        // Note: we don't eagerly clear `focused_input` on
        // press-down. The keyboard should stay up while the
        // user is panning or pressing buttons; only a
        // *confirmed tap* outside the keyboard's bounds should
        // dismiss it. That decision happens at `pointer_up`,
        // when we know whether the press resolved as a tap or
        // a drag.
        let Some((node, frame_x, _frame_y, frame_w, _frame_h)) = hit else {
            // No interactive hit. If there's a scrollview under
            // the pointer, drag-to-scroll starts immediately —
            // the empty-area-grabs-the-scroll iOS behavior.
            if let Some(sv) = scrollview_at {
                self.active_press = Some(ActivePress::Pan {
                    scrollview: sv,
                    last: ev.position,
                    last_time: Instant::now(),
                    velocity: (0.0, 0.0),
                    start: ev.position,
                });
            }
            return;
        };

        let action = pick_action(&node);
        match action {
            HitAction::Click(cb) => {
                set_state(&node, StateBits::PRESSED, true);
                self.active_press = Some(ActivePress::Click {
                    node: node.clone(),
                    action: ReleaseAction::Fire(cb),
                    over: true,
                    scrollview: scrollview_at,
                    start: ev.position,
                });
            }
            HitAction::ToggleFlip { on_change, .. } => {
                set_state(&node, StateBits::PRESSED, true);
                self.active_press = Some(ActivePress::Click {
                    node: node.clone(),
                    action: ReleaseAction::FlipToggle(on_change),
                    over: true,
                    scrollview: scrollview_at,
                    start: ev.position,
                });
            }
            HitAction::SliderJump {
                min,
                max,
                step,
                on_change,
            } => {
                let v = slider_value_from_pointer(
                    ev.position.0, frame_x, frame_w, min, max, step,
                );
                on_change(v);
                self.active_press = Some(ActivePress::SliderDrag { node: node.clone() });
            }
            HitAction::FocusInput => {
                self.focused_input = Some(node.clone());
            }
            HitAction::Nothing => {
                // No actionable widget, but if there's a scrollview
                // here, allow pan from this position.
                if let Some(sv) = scrollview_at {
                    self.active_press = Some(ActivePress::Pan {
                        scrollview: sv,
                        last: ev.position,
                        last_time: Instant::now(),
                        velocity: (0.0, 0.0),
                        start: ev.position,
                    });
                }
            }
        }

        // FocusInput may have set / replaced `focused_input`.
        // Reconcile the keyboard slide.
        self.sync_keyboard();
    }

    /// Pointer released. Ends the active interaction. For a click
    /// press, fires the release action only if the pointer is
    /// still over the captured node (the iOS "drag-off-to-cancel"
    /// model).
    pub fn pointer_up(&mut self, ev: PointerEvent) {
        let Some(press) = self.active_press.take() else { return };
        // Decide whether *this* release should dismiss the
        // on-screen keyboard. iOS rule (as restated by the user):
        // pans never dismiss; only confirmed taps outside the
        // keyboard's bounds dismiss. We compute `is_tap_outside`
        // for each release flavor and clear `focused_input` after
        // the press is finalized.
        let mut is_tap_outside = false;
        match press {
            ActivePress::SliderDrag { .. } => {
                // Slider drag has no fire-on-release semantics; the
                // last `pointer_move` already pushed the final
                // value. Treat as not-a-dismiss — user was
                // interacting with a control.
            }
            ActivePress::Pan {
                scrollview,
                last_time,
                velocity,
                start,
                ..
            } => {
                // Compute total motion since press-down. A short
                // motion means the user tapped (on empty area or
                // through a button-then-drag-cancel that never
                // moved past `PAN_THRESHOLD` — that path doesn't
                // upgrade to Pan, so we only see "real" pans here
                // and pans that started immediately on empty area
                // without any drag yet).
                let dx_total = ev.position.0 - start.0;
                let dy_total = ev.position.1 - start.1;
                let total_motion = (dx_total * dx_total + dy_total * dy_total).sqrt();
                let was_pan = total_motion >= PAN_THRESHOLD;

                if was_pan {
                    // Real pan: maybe hand off to momentum, don't
                    // dismiss the keyboard.
                    let now = Instant::now();
                    let stale = now.duration_since(last_time).as_millis()
                        > SCROLL_MOMENTUM_STALE_MS;
                    let speed_sq = velocity.0 * velocity.0 + velocity.1 * velocity.1;
                    let min_sq =
                        SCROLL_MOMENTUM_MIN_VELOCITY * SCROLL_MOMENTUM_MIN_VELOCITY;
                    if !stale && speed_sq > min_sq {
                        self.momentum = Some(ScrollMotion {
                            scrollview: scrollview.clone(),
                            kind: ScrollMotionKind::Coast { velocity },
                            last_tick: now,
                        });
                        crate::scheduler::request_redraw();
                    }
                    // If the user let go past the rubber-band
                    // overshoot region, replace any coast we
                    // just set up with a spring-back so the
                    // scrollview snaps to its bound. The
                    // spring-back path wins over coast when both
                    // could apply — momentum past a wall would
                    // just buzz.
                    self.maybe_spring_back(&scrollview, now);
                } else {
                    // Tap on empty area inside the scrollview —
                    // treat as a dismiss gesture.
                    is_tap_outside = true;
                }
            }
            ActivePress::Click { node, action, over, .. } => {
                set_state(&node, StateBits::PRESSED, false);
                if over {
                    fire_release(&node, action);
                    // Tapped a non-input widget (Click captures
                    // Pressable / Button / Toggle — not Slider,
                    // and TextInput follows the FocusInput path
                    // which never sets a Click press). Confirmed
                    // tap outside the keyboard → dismiss.
                    is_tap_outside = true;
                }
                // Release-outside the captured node (over=false)
                // is a cancelled tap. We don't fire and we don't
                // dismiss — the user dragged off the button,
                // they didn't commit.
            }
        }
        if is_tap_outside && self.focused_input.is_some() {
            self.focused_input = None;
            self.sync_keyboard();
        }
    }

    /// Scroll event (wheel / two-finger pan). Routed to the
    /// innermost `ScrollView` under `ev.position`. If no scroll
    /// container hits, the event is dropped — there's no
    /// document-level scroll (the root frame is always the
    /// viewport size).
    ///
    /// `ev.delta` carries the platform's wheel delta. We pass it
    /// through `apply_scroll` with a positive sign so wheel-down
    /// scrolls down — the "reverse" convention requested at the
    /// example level. Flip the sign here if you want
    /// natural-scrolling later.
    pub fn scroll(&mut self, ev: ScrollEvent) {
        let hit = {
            let backend = self.backend.borrow();
            let Some(root) = backend.root() else { return };
            find_scroll_view_at(&backend, &root, 0.0, 0.0, ev.position)
        };
        let Some(node) = hit else { return };
        // Vertical scrollviews accept both axes of wheel input
        // because mice with only a vertical wheel still need to
        // scroll — winit emits the user's twist on `.1`.
        let (dx, dy) = (ev.delta.0, ev.delta.1);
        // Wheel doesn't get rubber-band: there's no "release"
        // event to spring back from, and trackpad inertia
        // already mimics a fling visually.
        self.apply_scroll(&node, dx, dy, false);
    }

    /// Apply a delta in *content* space to a scrollview's offset.
    /// `rubber_band = true` lets the offset overshoot the
    /// `[0, max_offset]` range with diminishing returns — used
    /// during active pan. `false` hard-clamps — used by wheel
    /// and momentum-coast (which would otherwise spin past the
    /// edge forever).
    fn apply_scroll(&self, sv: &WgpuNode, dx: f32, dy: f32, rubber_band: bool) {
        let (max_x, max_y, span_x, span_y) = {
            let backend = self.backend.borrow();
            let frame = backend.layout.frame_of(sv.borrow().layout);
            let extent = scrollview_content_extent(&backend, sv);
            (
                (extent.0 - frame.width).max(0.0),
                (extent.1 - frame.height).max(0.0),
                frame.width.max(1.0),
                frame.height.max(1.0),
            )
        };
        if let NodeKind::ScrollView {
            horizontal,
            offset_x,
            offset_y,
        } = &mut sv.borrow_mut().kind
        {
            if *horizontal {
                let raw = *offset_x + dx + dy;
                *offset_x = if rubber_band {
                    rubberband(raw, 0.0, max_x, span_x)
                } else {
                    raw.clamp(0.0, max_x)
                };
            } else {
                let raw = *offset_y + dy;
                *offset_y = if rubber_band {
                    rubberband(raw, 0.0, max_y, span_y)
                } else {
                    raw.clamp(0.0, max_y)
                };
            }
        }
        crate::scheduler::request_redraw();
    }

    /// Inspect the scrollview's current offset and, if it's past
    /// either bound, install a spring-back motion that animates
    /// it to the nearest valid offset. Replaces any existing
    /// motion on the same scrollview — spring-back wins over
    /// coast at the wall (otherwise momentum would keep pressing
    /// against a clamped offset for the duration of its decay).
    fn maybe_spring_back(&mut self, sv: &WgpuNode, now: Instant) {
        let (max_x, max_y, cur_x, cur_y) = {
            let backend = self.backend.borrow();
            let frame = backend.layout.frame_of(sv.borrow().layout);
            let extent = scrollview_content_extent(&backend, sv);
            let max_x = (extent.0 - frame.width).max(0.0);
            let max_y = (extent.1 - frame.height).max(0.0);
            let (cx, cy) = match &sv.borrow().kind {
                NodeKind::ScrollView { offset_x, offset_y, .. } => (*offset_x, *offset_y),
                _ => return,
            };
            (max_x, max_y, cx, cy)
        };
        let target_x = cur_x.clamp(0.0, max_x);
        let target_y = cur_y.clamp(0.0, max_y);
        if (target_x - cur_x).abs() < f32::EPSILON
            && (target_y - cur_y).abs() < f32::EPSILON
        {
            return;
        }
        self.momentum = Some(ScrollMotion {
            scrollview: sv.clone(),
            kind: ScrollMotionKind::SpringBack {
                target: (target_x, target_y),
            },
            last_tick: now,
        });
        crate::scheduler::request_redraw();
    }

    /// Cancel an in-progress pointer interaction (window lost
    /// focus, OS-canceled touch). Clears PRESSED state without
    /// firing the release action.
    pub fn pointer_cancel(&mut self) {
        if let Some(ActivePress::Click { node, .. }) = self.active_press.take() {
            set_state(&node, StateBits::PRESSED, false);
        }
        // Slider / Pan: nothing to clean up beyond clearing the
        // active press (the `take()` above already did that for
        // any kind).
    }

    /// Hit-test `point` against `node`'s current absolute frame.
    /// Returns `true` if the deepest interactive hit at `point` is
    /// the same node — i.e. the user hasn't dragged onto a
    /// sibling. This is what flips the PRESSED state during a
    /// press-and-drag.
    fn pointer_over_node(&self, node: &WgpuNode, point: (f32, f32)) -> bool {
        let backend = self.backend.borrow();
        let Some(root) = backend.root() else { return false };
        let Some((hit, _, _, _, _)) = hit_test_node(&backend, &root, 0.0, 0.0, point) else {
            return false;
        };
        Rc::ptr_eq(&hit, node)
    }

    /// Process a key event against the currently focused input.
    /// No-op if nothing is focused. Returns `true` if the event
    /// produced a state change (useful for shells that want to
    /// decide whether to redraw).
    pub fn key(&mut self, event: &KeyEvent) -> bool {
        if !event.pressed {
            return false;
        }
        let Some(node) = self.focused_input.clone() else { return false };

        let (current_value, on_change) = {
            let data = node.borrow();
            match &data.kind {
                NodeKind::TextInput { value, on_change, .. } => {
                    (value.clone(), on_change.clone())
                }
                _ => return false,
            }
        };

        let new_value: Option<String> = match event.key {
            Key::Character => event.text.as_ref().and_then(|t| {
                let filtered: String = t.chars().filter(|c| !c.is_control()).collect();
                if filtered.is_empty() {
                    None
                } else {
                    let mut s = current_value.clone();
                    s.push_str(&filtered);
                    Some(s)
                }
            }),
            Key::Backspace => {
                if current_value.is_empty() {
                    None
                } else {
                    let mut s = current_value.clone();
                    s.pop();
                    Some(s)
                }
            }
            Key::Escape => {
                self.focused_input = None;
                self.sync_keyboard();
                return true;
            }
            // Cursor-movement keys are no-ops in the MVP — caret
            // is always at end-of-value. Shells should still pass
            // them through so future arrow / Home / End support
            // doesn't need a re-plumbing.
            Key::ArrowLeft
            | Key::ArrowRight
            | Key::ArrowUp
            | Key::ArrowDown
            | Key::Home
            | Key::End
            | Key::Delete
            | Key::Enter
            | Key::Tab
            | Key::Unknown => None,
        };

        if let Some(v) = new_value {
            on_change(v);
            true
        } else {
            false
        }
    }

    // ---------------- Internal -----------------------------------

    fn update_slider_drag(&self, node: &WgpuNode) {
        let backend = self.backend.borrow();
        let layout = node.borrow().layout;
        let frame = backend.layout.frame_of(layout);
        let frame_x = absolute_x(&backend, node);
        let frame_w = frame.width;
        let (min, max, step, on_change) = match &node.borrow().kind {
            NodeKind::Slider {
                min,
                max,
                step,
                on_change,
                ..
            } => (*min, *max, *step, on_change.clone()),
            _ => return,
        };
        drop(backend);
        let v = slider_value_from_pointer(self.pointer.0, frame_x, frame_w, min, max, step);
        on_change(v);
    }
}

/// Push a state bit toggle through the node's framework-supplied
/// setter, if one is installed. No-op when the node's stylesheet
/// has no state overlays (the setter is only attached when there's
/// something to drive).
fn set_state(node: &WgpuNode, bit: StateBits, on: bool) {
    let setter = node.borrow().state_setter.clone();
    if let Some(setter) = setter {
        setter(bit, on);
    }
}

/// Fire the captured release action for a click press. Toggle
/// reads the node's *current* value at fire time so an external
/// signal flip during the press doesn't cause a wrong-direction
/// toggle.
fn fire_release(node: &WgpuNode, action: ReleaseAction) {
    match action {
        ReleaseAction::Fire(cb) => cb(),
        ReleaseAction::FlipToggle(cb) => {
            let current = match &node.borrow().kind {
                NodeKind::Toggle { value, .. } => *value,
                _ => return,
            };
            cb(!current);
        }
    }
}

// ---------------------------------------------------------------------------
// Free-standing helpers shared with the renderer walk in `app.rs`.
// ---------------------------------------------------------------------------

enum HitAction {
    Click(Rc<dyn Fn()>),
    ToggleFlip {
        on_change: Rc<dyn Fn(bool)>,
    },
    SliderJump {
        min: f32,
        max: f32,
        step: Option<f32>,
        on_change: Rc<dyn Fn(f32)>,
    },
    FocusInput,
    Nothing,
}

fn pick_action(node: &WgpuNode) -> HitAction {
    match &node.borrow().kind {
        NodeKind::Pressable { on_click } => HitAction::Click(on_click.clone()),
        NodeKind::Button { on_click, .. } => HitAction::Click(on_click.clone()),
        NodeKind::Toggle { on_change, .. } => HitAction::ToggleFlip {
            on_change: on_change.clone(),
        },
        NodeKind::Slider {
            min,
            max,
            step,
            on_change,
            ..
        } => HitAction::SliderJump {
            min: *min,
            max: *max,
            step: *step,
            on_change: on_change.clone(),
        },
        NodeKind::TextInput { .. } => HitAction::FocusInput,
        _ => HitAction::Nothing,
    }
}

/// Returns the innermost `ScrollView` containing `point`, or
/// `None`. Walks the tree the same way the renderer does — when
/// it enters a `ScrollView`, children are descended through
/// `point - offset` so a click at the user's cursor lines up
/// with the visually scrolled content.
fn find_scroll_view_at(
    backend: &WgpuBackend,
    node: &WgpuNode,
    parent_x: f32,
    parent_y: f32,
    point: (f32, f32),
) -> Option<WgpuNode> {
    let frame = backend.layout.frame_of(node.borrow().layout);
    let x = parent_x + frame.x;
    let y = parent_y + frame.y;
    let inside = point.0 >= x
        && point.0 <= x + frame.width
        && point.1 >= y
        && point.1 <= y + frame.height;
    if !inside {
        return None;
    }
    // Descend children first — favor the innermost scrollview
    // when nested.
    let (child_origin, this_is_scroll) = match &node.borrow().kind {
        NodeKind::ScrollView { offset_x, offset_y, .. } => ((x - *offset_x, y - *offset_y), true),
        _ => ((x, y), false),
    };
    let children = node.borrow().children.clone();
    for child in children.iter().rev() {
        if let Some(hit) = find_scroll_view_at(backend, child, child_origin.0, child_origin.1, point) {
            return Some(hit);
        }
    }
    if this_is_scroll {
        Some(node.clone())
    } else {
        None
    }
}

/// Returns the deepest interactive node containing `point` along
/// with its absolute *visual* frame (x, y, w, h), or None.
///
/// Hit-test uses an *inflated* rectangle for small native widgets:
/// any interactive node whose visual bounds are below the iOS
/// 44pt minimum touch target is expanded outward symmetrically.
/// The reported frame is still the visual one — callers that need
/// the widget's actual pixel rect (slider drag value mapping, for
/// instance) get the truth, while the touch zone is generous.
///
/// Containers (a plain `View`) are walked but never hit-tested
/// inflated — only the leaf-interactive node gets the bonus.
pub(crate) fn hit_test_node(
    backend: &WgpuBackend,
    node: &WgpuNode,
    parent_x: f32,
    parent_y: f32,
    point: (f32, f32),
) -> Option<(WgpuNode, f32, f32, f32, f32)> {
    let frame = backend.layout.frame_of(node.borrow().layout);
    let x = parent_x + frame.x;
    let y = parent_y + frame.y;
    let w = frame.width;
    let h = frame.height;

    // For interactive leaves, expand the touch rect outward to at
    // least the iOS minimum. For containers, use the literal frame
    // so we still recurse into children at their proper bounds.
    let kind_matches = matches!(
        &node.borrow().kind,
        NodeKind::Pressable { .. }
            | NodeKind::Button { .. }
            | NodeKind::Toggle { .. }
            | NodeKind::Slider { .. }
            | NodeKind::TextInput { .. }
    );
    // Two components per axis:
    //   - the `44pt - size` correction so tiny controls grow to
    //     the iOS minimum touch target,
    //   - a constant `HIT_SLOP` on top, so even controls that are
    //     already wider than 44pt accept a few pixels of overshoot
    //     past their visual edge (UIKit apps do this routinely via
    //     `hitTest` overrides).
    let inflate_w = if kind_matches {
        ((IOS_MIN_HIT_TARGET - w) * 0.5).max(0.0) + HIT_SLOP
    } else {
        0.0
    };
    let inflate_h = if kind_matches {
        ((IOS_MIN_HIT_TARGET - h) * 0.5).max(0.0) + HIT_SLOP
    } else {
        0.0
    };
    let hx0 = x - inflate_w;
    let hy0 = y - inflate_h;
    let hx1 = x + w + inflate_w;
    let hy1 = y + h + inflate_h;
    let inside = point.0 >= hx0 && point.0 <= hx1 && point.1 >= hy0 && point.1 <= hy1;
    if !inside {
        return None;
    }

    // When entering a ScrollView, descend with the scroll offset
    // applied so children's hit rects line up with what the user
    // sees on screen.
    let (child_origin_x, child_origin_y) = match &node.borrow().kind {
        NodeKind::ScrollView { offset_x, offset_y, .. } => (x - *offset_x, y - *offset_y),
        _ => (x, y),
    };

    // Children first (deeper hits beat shallow ones).
    let children = node.borrow().children.clone();
    for child in children.iter().rev() {
        if let Some(hit) = hit_test_node(backend, child, child_origin_x, child_origin_y, point) {
            // A child reported its hit in *scrolled* coordinates;
            // they're still valid absolute logical-px positions for
            // the slider drag math etc., so just propagate.
            return Some(hit);
        }
    }

    if kind_matches {
        // Return the visual frame, not the inflated one. The
        // slider value mapping uses these coordinates to map
        // pointer x → value, and we want it relative to the
        // *visible* track.
        Some((node.clone(), x, y, w, h))
    } else {
        None
    }
}

/// Sum the layout x-offsets from the root to `node`. Used by
/// slider drag — Taffy frames are parent-relative.
fn absolute_x(backend: &WgpuBackend, node: &WgpuNode) -> f32 {
    let target = node.borrow().layout;
    let Some(root) = backend.roots.first() else { return 0.0 };
    let mut accum = 0.0;
    if walk_for_x(backend, root, target, 0.0, &mut accum) {
        accum
    } else {
        0.0
    }
}

fn walk_for_x(
    backend: &WgpuBackend,
    node: &WgpuNode,
    target: LayoutNode,
    parent_x: f32,
    out: &mut f32,
) -> bool {
    let frame = backend.layout.frame_of(node.borrow().layout);
    let x = parent_x + frame.x;
    if node.borrow().layout == target {
        *out = x;
        return true;
    }
    let children: Vec<WgpuNode> = node.borrow().children.clone();
    for child in &children {
        if walk_for_x(backend, child, target, x, out) {
            return true;
        }
    }
    false
}

/// Total extent (width, height) of `sv`'s laid-out children in
/// the scrollview's local content space. Used both for clamping
/// scroll offsets and for sizing the scrollbar thumb. Computed
/// from each direct child's Taffy frame.
pub(crate) fn scrollview_content_extent(
    backend: &WgpuBackend,
    sv: &WgpuNode,
) -> (f32, f32) {
    let children = sv.borrow().children.clone();
    let mut max_x: f32 = 0.0;
    let mut max_y: f32 = 0.0;
    for c in &children {
        let f = backend.layout.frame_of(c.borrow().layout);
        max_x = max_x.max(f.x + f.width);
        max_y = max_y.max(f.y + f.height);
    }
    (max_x, max_y)
}

/// Locate the absolute `(y, height)` of the node with layout id
/// `target`, plus the innermost `ScrollView` ancestor enclosing
/// it. Returns `None` if the target isn't in the tree. Single
/// pre-order walk, applies scroll offsets so the returned y is
/// the user's-eye position (matching `walk` in the renderer).
fn find_input_in_scrollview(
    backend: &WgpuBackend,
    node: &WgpuNode,
    target: LayoutNode,
    parent_x: f32,
    parent_y: f32,
    enclosing_sv: Option<WgpuNode>,
) -> Option<(f32, f32, WgpuNode)> {
    let frame = backend.layout.frame_of(node.borrow().layout);
    let x = parent_x + frame.x;
    let y = parent_y + frame.y;

    if node.borrow().layout == target {
        return enclosing_sv.map(|sv| (y, frame.height, sv));
    }

    // Descend with scroll offset applied + scrollview tracking.
    let (child_x, child_y, new_enclosing) = match &node.borrow().kind {
        NodeKind::ScrollView { offset_x, offset_y, .. } => {
            (x - *offset_x, y - *offset_y, Some(node.clone()))
        }
        _ => (x, y, enclosing_sv),
    };
    let children = node.borrow().children.clone();
    for c in &children {
        if let Some(found) = find_input_in_scrollview(
            backend,
            c,
            target,
            child_x,
            child_y,
            new_enclosing.clone(),
        ) {
            return Some(found);
        }
    }
    None
}

/// iOS rubber-band saturation. `v` is the raw offset, `[min,
/// max]` the valid range, `span` the scrollview's main-axis
/// size. Inside the range, returns `v` unchanged. Past either
/// bound, damps the overshoot with `overshoot / (1 + overshoot
/// * resistance / span)` — a hyperbolic saturation that lets
/// the user drag arbitrarily far while the visible motion
/// shrinks asymptotically.
fn rubberband(v: f32, min: f32, max: f32, span: f32) -> f32 {
    if v >= min && v <= max {
        return v;
    }
    let (bound, overshoot, sign) = if v < min {
        (min, min - v, -1.0)
    } else {
        (max, v - max, 1.0)
    };
    let damped = overshoot / (1.0 + overshoot * SCROLL_RUBBERBAND_RESISTANCE / span.max(1.0));
    bound + sign * damped
}

/// Tiny easing-curve helper for the keyboard slide. The full
/// animation engine has the same suite (`animation::Animator`),
/// but the keyboard's animation isn't node-bound so it lives on
/// `Host` directly; this is the matching curve evaluator.
fn ease(easing: Easing, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => t,
        Easing::EaseIn => t * t,
        Easing::EaseOut => {
            let inv = 1.0 - t;
            1.0 - inv * inv * inv
        }
        Easing::EaseInOut => {
            if t < 0.5 {
                2.0 * t * t
            } else {
                let f = -2.0 * t + 2.0;
                1.0 - f * f * 0.5
            }
        }
        // CSS default ≈ ease-out cubic; close enough here.
        Easing::Ease => {
            let inv = 1.0 - t;
            1.0 - inv * inv * inv
        }
        Easing::CubicBezier(_, _, _, _) => t,
    }
}

// ---------------------------------------------------------------------------
// EventSink — the formal contract a native shell drives this
// render backend through. Every method delegates to the
// matching inherent fn on `Host` (the bodies above) so we keep
// one source of truth for the actual behavior.
// ---------------------------------------------------------------------------

impl EventSink for Host {
    fn pointer_down(&mut self, ev: PointerEvent) {
        Host::pointer_down(self, ev)
    }
    fn pointer_move(&mut self, ev: PointerEvent) {
        Host::pointer_move(self, ev)
    }
    fn pointer_up(&mut self, ev: PointerEvent) {
        Host::pointer_up(self, ev)
    }
    fn pointer_cancel(&mut self) {
        Host::pointer_cancel(self)
    }
    fn scroll(&mut self, ev: ScrollEvent) {
        Host::scroll(self, ev)
    }
    fn key(&mut self, ev: &KeyEvent) -> bool {
        Host::key(self, ev)
    }
    fn set_viewport(&mut self, w: f32, h: f32) {
        Host::set_viewport(self, w, h)
    }
    fn tick(&mut self, now: Instant) -> bool {
        Host::tick(self, now)
    }
}

/// Map a pointer x position to a slider value, honoring optional
/// step quantization.
fn slider_value_from_pointer(
    point_x: f32,
    frame_x: f32,
    frame_w: f32,
    min: f32,
    max: f32,
    step: Option<f32>,
) -> f32 {
    let inset = SLIDER_THUMB_SIZE * 0.5;
    let track_x = frame_x + inset;
    let track_w = (frame_w - inset * 2.0).max(1.0);
    let t = ((point_x - track_x) / track_w).clamp(0.0, 1.0);
    let raw = min + t * (max - min);
    match step {
        Some(s) if s > 0.0 => {
            let steps = ((raw - min) / s).round();
            (min + steps * s).clamp(min, max)
        }
        _ => raw,
    }
}
