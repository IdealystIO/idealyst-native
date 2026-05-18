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
use std::rc::Rc;

use framework_core::{render, ColorScheme, Owner, Primitive, StateBits};
use glyphon::FontSystem;
use native_layout::LayoutNode;

use crate::app::SimulatedPlatform;
use crate::backend_impl::WgpuBackend;
use crate::input::{Key, KeyEvent, PointerButton, PointerEvent};
use crate::node::{NodeKind, WgpuNode, HIT_SLOP, IOS_MIN_HIT_TARGET, SLIDER_THUMB_SIZE};
use crate::text::TextStore;

pub struct Host {
    backend: Rc<RefCell<WgpuBackend>>,
    text: Rc<RefCell<TextStore>>,
    font_system: Rc<RefCell<FontSystem>>,
    platform: SimulatedPlatform,
    /// Currently keyboard-focused TextInput, if any. Cleared on
    /// click outside any input or on Esc.
    focused_input: Option<WgpuNode>,
    /// Active pointer-down interaction, if any. iOS-style: a press
    /// captures a node and a release action; whether the action
    /// fires depends on whether the pointer is still inside the
    /// node when the pointer comes up. Cleared on pointer-up /
    /// pointer-cancel.
    active_press: Option<ActivePress>,
    /// Most recent pointer position in logical px. Updated by
    /// every pointer event so slider drag has fresh coordinates
    /// when the platform only delivers a `down` (no `move`).
    pointer: (f32, f32),
    /// Framework reactive scopes. Held so they outlive the host;
    /// cleared on `unmount` or drop.
    _owner: Option<Owner>,
}

/// What a pointer-down created. Lives only while the pointer is
/// held; cleared on up/cancel.
enum ActivePress {
    /// Pressable / Button / Toggle: press → maybe-fire on release.
    Click {
        node: WgpuNode,
        action: ReleaseAction,
        /// `true` while the pointer is over the captured node.
        /// Toggles as the pointer moves in/out of the bounds; the
        /// release action fires only if `over` is true at up.
        over: bool,
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
    pub fn new(platform: SimulatedPlatform, color_scheme: ColorScheme) -> Self {
        let text = Rc::new(RefCell::new(TextStore::new()));
        let font_system = Rc::new(RefCell::new(FontSystem::new()));
        let backend = Rc::new(RefCell::new(WgpuBackend::new(
            text.clone(),
            font_system.clone(),
            color_scheme,
        )));
        Self {
            backend,
            text,
            font_system,
            platform,
            focused_input: None,
            active_press: None,
            pointer: (0.0, 0.0),
            _owner: None,
        }
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
    pub fn platform(&self) -> SimulatedPlatform { self.platform }
    pub fn focused_input_layout(&self) -> Option<LayoutNode> {
        self.focused_input.as_ref().map(|n| n.borrow().layout)
    }

    /// Advance the animator's clock. Purges completed tweens.
    /// Returns `true` if any tweens are still active — the caller
    /// (the shell's render loop) should `request_redraw` so the
    /// next frame samples the next step.
    pub fn tick_animations(&self, now: std::time::Instant) -> bool {
        self.backend.borrow_mut().animator.tick(now)
    }

    // ---------------- Event entry points -------------------------

    /// Pointer moved. Updates the cached position, drives slider
    /// drag value updates, and toggles the PRESSED state on any
    /// active click-press as the pointer enters / leaves its
    /// captured node (so `state pressed { ... }` overlays only
    /// apply while the press is still "live").
    pub fn pointer_move(&mut self, ev: PointerEvent) {
        self.pointer = ev.position;
        // Take the press out (rather than `as_mut`) so we can
        // re-enter `self` for hit-testing while we hold a clone of
        // the captured node. Put it back when we're done.
        let Some(press) = self.active_press.take() else { return };
        match press {
            ActivePress::SliderDrag { node } => {
                self.update_slider_drag(&node);
                self.active_press = Some(ActivePress::SliderDrag { node });
            }
            ActivePress::Click { node, action, over } => {
                let new_over = self.pointer_over_node(&node, ev.position);
                if new_over != over {
                    set_state(&node, StateBits::PRESSED, new_over);
                }
                self.active_press = Some(ActivePress::Click {
                    node,
                    action,
                    over: new_over,
                });
            }
        }
    }

    /// Pointer pressed. Picks an interaction based on what's
    /// under the pointer:
    /// - Pressable / Button → capture; fire on release-inside
    /// - Toggle           → capture; flip on release-inside
    /// - Slider           → capture; emit initial value, start drag
    /// - TextInput        → focus immediately (iOS keyboard model)
    /// - nothing          → drop any active TextInput focus
    pub fn pointer_down(&mut self, ev: PointerEvent) {
        if !matches!(ev.button, PointerButton::Primary) {
            return;
        }
        self.pointer = ev.position;
        let hit = {
            let backend = self.backend.borrow();
            let Some(root) = backend.root() else {
                self.focused_input = None;
                return;
            };
            hit_test_node(&backend, &root, 0.0, 0.0, ev.position)
        };
        let Some((node, frame_x, _frame_y, frame_w, _frame_h)) = hit else {
            self.focused_input = None;
            return;
        };

        let action = pick_action(&node);

        if !matches!(action, HitAction::FocusInput) {
            self.focused_input = None;
        }
        match action {
            HitAction::Click(cb) => {
                set_state(&node, StateBits::PRESSED, true);
                self.active_press = Some(ActivePress::Click {
                    node: node.clone(),
                    action: ReleaseAction::Fire(cb),
                    over: true,
                });
            }
            HitAction::ToggleFlip { on_change, .. } => {
                set_state(&node, StateBits::PRESSED, true);
                self.active_press = Some(ActivePress::Click {
                    node: node.clone(),
                    action: ReleaseAction::FlipToggle(on_change),
                    over: true,
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
            HitAction::Nothing => {}
        }
    }

    /// Pointer released. Ends the active interaction. For a click
    /// press, fires the release action only if the pointer is
    /// still over the captured node (the iOS "drag-off-to-cancel"
    /// model).
    pub fn pointer_up(&mut self, _ev: PointerEvent) {
        let Some(press) = self.active_press.take() else { return };
        match press {
            ActivePress::SliderDrag { .. } => {
                // Slider drag has no fire-on-release semantics; the
                // last `pointer_move` already pushed the final
                // value.
            }
            ActivePress::Click { node, action, over } => {
                set_state(&node, StateBits::PRESSED, false);
                if over {
                    fire_release(&node, action);
                }
            }
        }
    }

    /// Cancel an in-progress pointer interaction (window lost
    /// focus, OS-canceled touch). Clears PRESSED state without
    /// firing the release action.
    pub fn pointer_cancel(&mut self) {
        if let Some(ActivePress::Click { node, .. }) = self.active_press.take() {
            set_state(&node, StateBits::PRESSED, false);
        }
        // Slider drag: nothing to clean up beyond clearing the press.
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

    // Children first (deeper hits beat shallow ones).
    let children = node.borrow().children.clone();
    for child in children.iter().rev() {
        if let Some(hit) = hit_test_node(backend, child, x, y, point) {
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
