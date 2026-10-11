//! Android `MotionEvent` → app-level pointer vocabulary
//! ([`AppPointerEvent`]), plus the app-situation decisions (reduce motion,
//! primary pointer). Pure functions over the integers/floats the Kotlin
//! `RustGlobalPointerListener` / `RustAppStateObserver` pass across JNI,
//! un-gated like `app_key_policy` so the tests run on the host. The JNI
//! trampolines that drive it live in `imp/jni_exports.rs`
//! (`nativePointerMotion`, `nativeAppPrimaryPointer`, …); the Kotlin side is
//! deliberately thin (it forwards raw `MotionEvent` fields) so every decision
//! below is covered here.
//!
//! # Buttons come from `getButtonState()` diffs, not from action codes
//!
//! A mouse press arrives as TWO events in an order that differs by event
//! class: the touch-stream `ACTION_DOWN` (which is what triggers focus change,
//! long-press and the context menu inside `View.onTouchEvent`) and then the
//! generic `ACTION_BUTTON_PRESS` carrying `getActionButton()`; a second button
//! pressed during a drag arrives ONLY as `ACTION_BUTTON_PRESS`; a release is
//! `ACTION_BUTTON_RELEASE` then `ACTION_UP`. Every one of them carries the
//! post-event `getButtonState()`. [`PointerTracker`] therefore diffs the
//! button state against the last one it saw and emits a `Down`/`Up` per
//! changed bit — whichever event shows the change first. That makes the
//! `Down` arrive on the `ACTION_DOWN` itself, so a `PreventDefault` answer can
//! consume the event that would otherwise open the context menu / move focus.
//!
//! A stylus touching the screen (and an injected mouse tap) reports
//! `buttonState == 0` while in contact; the contact itself counts as the
//! primary button for those tools.
//!
//! # Units
//!
//! - Positions: the listener passes px relative to the app root; divided by
//!   the display density into logical px (dp), the unit every frame and
//!   touch coordinate in this backend uses.
//! - Wheel: `AXIS_VSCROLL`/`AXIS_HSCROLL` are in notches (fractional for
//!   high-resolution wheels and touchpads). [`WHEEL_PX_PER_NOTCH`] converts.
//!   Android's `AXIS_VSCROLL` is POSITIVE when scrolling UP (wheel away from
//!   the user); the framework uses the web `deltaY` sign (positive = wheel
//!   toward the user = content scrolls down), so vertical is negated.
//!   `AXIS_HSCROLL` is positive for "scroll right" — already the web
//!   `deltaX` sign.
//! - Locked (pointer capture) motion: `SOURCE_MOUSE_RELATIVE` events carry
//!   the raw, unaccelerated mouse counts in `getX()`/`getY()`. They are
//!   reported 1:1 as `dx`/`dy` — NOT divided by density — matching the raw
//!   counts web (`unadjustedMovement`) and Windows raw input report; scaling
//!   counts by screen density would make look-sensitivity depend on the
//!   phone's panel.

use runtime_shared::app_state::PointerKind;
use runtime_shared::pointer_input::{AppPointerEvent, MouseButton, PointerPhase};

use crate::app_key_policy::Modifiers;

/// Logical px per wheel notch — the framework-wide value
/// (`runtime_shared::pointer_input::WHEEL_NOTCH_PX`), so a wheel turn moves
/// app content the same distance on every platform. (AOSP's own lists use
/// `config_verticalScrollFactor`, 64 dp; app-level wheel deltas follow the
/// framework, not that.)
pub(crate) const WHEEL_PX_PER_NOTCH: f32 = runtime_shared::pointer_input::WHEEL_NOTCH_PX;

// `MotionEvent.ACTION_*` (stable ABI, `MotionEvent.java`).
pub(crate) const ACTION_DOWN: i32 = 0;
pub(crate) const ACTION_UP: i32 = 1;
pub(crate) const ACTION_MOVE: i32 = 2;
pub(crate) const ACTION_CANCEL: i32 = 3;
pub(crate) const ACTION_HOVER_MOVE: i32 = 7;
pub(crate) const ACTION_SCROLL: i32 = 8;
pub(crate) const ACTION_HOVER_ENTER: i32 = 9;
pub(crate) const ACTION_HOVER_EXIT: i32 = 10;
pub(crate) const ACTION_BUTTON_PRESS: i32 = 11;
pub(crate) const ACTION_BUTTON_RELEASE: i32 = 12;

// `MotionEvent.BUTTON_*`.
const BUTTON_PRIMARY: i32 = 0x01;
const BUTTON_SECONDARY: i32 = 0x02;
const BUTTON_TERTIARY: i32 = 0x04;
const BUTTON_BACK: i32 = 0x08;
const BUTTON_FORWARD: i32 = 0x10;
const BUTTON_STYLUS_PRIMARY: i32 = 0x20;
const BUTTON_STYLUS_SECONDARY: i32 = 0x40;

// `MotionEvent.TOOL_TYPE_*`.
pub(crate) const TOOL_TYPE_FINGER: i32 = 1;
pub(crate) const TOOL_TYPE_STYLUS: i32 = 2;
pub(crate) const TOOL_TYPE_MOUSE: i32 = 3;
pub(crate) const TOOL_TYPE_ERASER: i32 = 4;

// `InputDevice.SOURCE_*` (class bits in the low byte).
#[cfg_attr(not(test), allow(dead_code))] // tests only: the finger source
pub(crate) const SOURCE_TOUCHSCREEN: i32 = 0x0000_1002;
pub(crate) const SOURCE_MOUSE: i32 = 0x0000_2002;
pub(crate) const SOURCE_STYLUS: i32 = 0x0000_4002;
pub(crate) const SOURCE_MOUSE_RELATIVE: i32 = 0x0002_0004;
pub(crate) const SOURCE_TOUCHPAD: i32 = 0x0010_0008;

fn from_source(source: i32, wanted: i32) -> bool {
    source & wanted == wanted
}

/// Whether a `MotionEvent` comes from a mouse-like device the app-level
/// pointer layer reports: a mouse, a touchpad driving the cursor (Android 13+
/// reports its pointer as `SOURCE_MOUSE` with `TOOL_TYPE_FINGER`), a stylus /
/// eraser, or a captured (relative) mouse or touchpad. A finger on the touch
/// screen is NOT — finger touches belong to the view-level touch APIs.
pub(crate) fn is_pointer_device(source: i32, tool_type: i32) -> bool {
    match tool_type {
        TOOL_TYPE_MOUSE | TOOL_TYPE_STYLUS | TOOL_TYPE_ERASER => true,
        _ => {
            from_source(source, SOURCE_MOUSE)
                || from_source(source, SOURCE_MOUSE_RELATIVE)
                || from_source(source, SOURCE_TOUCHPAD)
                || (from_source(source, SOURCE_STYLUS) && tool_type != TOOL_TYPE_FINGER)
        }
    }
}

/// `MotionEvent.getButtonState()` → framework button bits
/// ([`MouseButton::bit`]). Stylus barrel buttons map like the web's pen
/// buttons: the first barrel button is the secondary ("right") button, the
/// second is middle.
pub(crate) fn button_bits(button_state: i32) -> u8 {
    let mut bits = 0u8;
    if button_state & BUTTON_PRIMARY != 0 {
        bits |= MouseButton::Primary.bit();
    }
    if button_state & (BUTTON_SECONDARY | BUTTON_STYLUS_PRIMARY) != 0 {
        bits |= MouseButton::Secondary.bit();
    }
    if button_state & (BUTTON_TERTIARY | BUTTON_STYLUS_SECONDARY) != 0 {
        bits |= MouseButton::Middle.bit();
    }
    if button_state & BUTTON_BACK != 0 {
        bits |= MouseButton::Back.bit();
    }
    if button_state & BUTTON_FORWARD != 0 {
        bits |= MouseButton::Forward.bit();
    }
    bits
}

const BUTTON_ORDER: [MouseButton; 5] = [
    MouseButton::Primary,
    MouseButton::Secondary,
    MouseButton::Middle,
    MouseButton::Back,
    MouseButton::Forward,
];

/// `AXIS_VSCROLL`/`AXIS_HSCROLL` notches → web-convention wheel px
/// `(wheel_dx, wheel_dy)`. See the module docs for the sign.
pub(crate) fn wheel_px(hscroll: f32, vscroll: f32) -> (f32, f32) {
    (hscroll * WHEEL_PX_PER_NOTCH, -vscroll * WHEEL_PX_PER_NOTCH)
}

/// Screen px → logical px. A non-positive density (a failed read) is
/// treated as 1 rather than producing infinities.
pub(crate) fn px_to_dp(px: f32, density: f32) -> f32 {
    if density > 0.0 {
        px / density
    } else {
        px
    }
}

/// One `MotionEvent` as the Kotlin listener forwards it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RawMotion {
    pub(crate) action: i32,
    pub(crate) button_state: i32,
    pub(crate) source: i32,
    pub(crate) tool_type: i32,
    /// Px relative to the app root — or, for a `captured` event, the
    /// event's own `getX()`/`getY()` (relative counts for a mouse, absolute
    /// touchpad coordinates for a captured touchpad).
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) vscroll: f32,
    pub(crate) hscroll: f32,
    pub(crate) meta_state: i32,
    pub(crate) density: f32,
    /// Delivered through `OnCapturedPointerListener` (pointer lock).
    pub(crate) captured: bool,
    /// Arrived through `dispatchTouchEvent` (the touch stream) rather than
    /// `dispatchGenericMotionEvent`.
    pub(crate) touch_stream: bool,
}

impl RawMotion {
    /// Blank event for tests and the trampoline's struct-update syntax.
    #[cfg_attr(target_os = "android", allow(dead_code))]
    pub(crate) fn new(action: i32, source: i32, tool_type: i32) -> Self {
        RawMotion {
            action,
            button_state: 0,
            source,
            tool_type,
            x: 0.0,
            y: 0.0,
            vscroll: 0.0,
            hscroll: 0.0,
            meta_state: 0,
            density: 1.0,
            captured: false,
            touch_stream: false,
        }
    }
}

/// What to do with one `MotionEvent`: the framework events to deliver, in
/// order. The caller delivers them, then calls [`PointerTracker::settle`]
/// with whether any came back `PreventDefault`.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct Translation {
    pub(crate) events: Vec<AppPointerEvent>,
    /// The event belongs to a press stream already being swallowed (a
    /// prevented `Down` earlier) — consume it regardless of the outcome.
    pub(crate) swallow: bool,
}

/// Per-source state: last position, held buttons, and whether the current
/// press stream is being swallowed.
#[derive(Debug, Default)]
pub(crate) struct PointerTracker {
    buttons: u8,
    position: Option<(f32, f32)>,
    /// A `Down` was prevented: consume every event of this press stream
    /// (touch-stream `MOVE`/`UP`, the generic `BUTTON_PRESS`/`RELEASE`), so
    /// views never see half a gesture and `View.performContextClick` (fired
    /// from `ACTION_BUTTON_PRESS`) can't open a menu either.
    swallowing: bool,
    /// A touch-stream `ACTION_DOWN` started a press stream that hasn't
    /// ended (`ACTION_UP`/`CANCEL`) yet.
    stream_open: bool,
    /// Last absolute coordinate of a captured TOUCHPAD finger (captured
    /// touchpads report absolute pad coordinates, not deltas).
    captured_pad: Option<(f32, f32)>,
}

impl PointerTracker {
    /// Translate one raw event. `None` = not a mouse-like event; pass it
    /// through untouched.
    pub(crate) fn translate(&mut self, raw: &RawMotion) -> Option<Translation> {
        if !is_pointer_device(raw.source, raw.tool_type) {
            return None;
        }
        let m = Modifiers::from_meta_state(raw.meta_state);
        let (mut x, mut y) = self.position.unwrap_or((0.0, 0.0));
        let (mut dx, mut dy) = (0.0f32, 0.0f32);

        if raw.captured {
            if from_source(raw.source, SOURCE_TOUCHPAD) && !from_source(raw.source, SOURCE_MOUSE_RELATIVE) {
                // Absolute pad coordinates: delta against the previous
                // sample of the same contact.
                match raw.action {
                    ACTION_MOVE => {
                        if let Some((px, py)) = self.captured_pad {
                            dx = raw.x - px;
                            dy = raw.y - py;
                        }
                        self.captured_pad = Some((raw.x, raw.y));
                    }
                    ACTION_DOWN => self.captured_pad = Some((raw.x, raw.y)),
                    ACTION_UP | ACTION_CANCEL => self.captured_pad = None,
                    _ => {}
                }
            } else if matches!(raw.action, ACTION_MOVE | ACTION_HOVER_MOVE) {
                dx = raw.x;
                dy = raw.y;
            }
        } else if raw.action != ACTION_SCROLL || self.position.is_none() {
            // ACTION_SCROLL carries the cursor position too, but scrolling
            // isn't movement; only seed the position from it.
            let p = (px_to_dp(raw.x, raw.density), px_to_dp(raw.y, raw.density));
            if let Some((ox, oy)) = self.position {
                dx = p.0 - ox;
                dy = p.1 - oy;
            }
            x = p.0;
            y = p.1;
            self.position = Some(p);
        }

        let stamp = |phase: PointerPhase| AppPointerEvent {
            shift: m.shift,
            ctrl: m.ctrl,
            alt: m.alt,
            meta: m.meta,
            ..AppPointerEvent::new(phase, x, y)
        };

        let mut events = Vec::new();
        let moved = dx != 0.0 || dy != 0.0;
        if moved && raw.action != ACTION_HOVER_EXIT {
            events.push(AppPointerEvent { dx, dy, ..stamp(PointerPhase::Move) });
        }

        // Buttons: diff the post-event state.
        let contact = raw.touch_stream
            && matches!(raw.action, ACTION_DOWN | ACTION_MOVE)
            && raw.button_state == 0
            && raw.tool_type != TOOL_TYPE_FINGER;
        let mut now = button_bits(raw.button_state);
        if contact {
            now |= MouseButton::Primary.bit();
        }
        if raw.touch_stream && matches!(raw.action, ACTION_UP | ACTION_CANCEL) {
            // End of contact: nothing the stream pressed is still held.
            now = button_bits(raw.button_state);
        }
        // A hover event means no contact; trust its (usually 0) state.
        let before = self.buttons;
        let released = before & !now;
        let pressed = now & !before;
        for b in BUTTON_ORDER {
            if released & b.bit() != 0 {
                events.push(AppPointerEvent { button: Some(b), ..stamp(PointerPhase::Up) });
            }
        }
        for b in BUTTON_ORDER {
            if pressed & b.bit() != 0 {
                events.push(AppPointerEvent { button: Some(b), ..stamp(PointerPhase::Down) });
            }
        }
        self.buttons = now;

        if raw.action == ACTION_SCROLL {
            let (wx, wy) = wheel_px(raw.hscroll, raw.vscroll);
            if wx != 0.0 || wy != 0.0 {
                events.push(AppPointerEvent { wheel_dx: wx, wheel_dy: wy, ..stamp(PointerPhase::Wheel) });
            }
        }

        let swallow = self.swallowing
            && (raw.touch_stream || matches!(raw.action, ACTION_BUTTON_PRESS | ACTION_BUTTON_RELEASE));
        Some(Translation { events, swallow })
    }

    /// Record the outcome of delivering `translation` for `raw` and decide
    /// whether to consume the platform event. `prevented` = any delivered
    /// event came back `PreventDefault`.
    ///
    /// `PreventDefault` only means something on a `Down` (no focus change,
    /// no context menu) and a `Wheel` (no scrolling) — the contract. A
    /// prevented `Down` that STARTS a press stream swallows the whole stream
    /// so views never see half a gesture. A prevented `Down` for a second
    /// button mid-stream (the views already have the stream) consumes only
    /// that `ACTION_BUTTON_PRESS` — consuming the stream's `ACTION_UP` would
    /// leave a view stuck pressed.
    pub(crate) fn settle(&mut self, raw: &RawMotion, translation: &Translation, prevented: bool) -> bool {
        let has = |phase| translation.events.iter().any(|e| e.phase == phase);
        let had_down = has(PointerPhase::Down);
        let starts_stream = raw.touch_stream && raw.action == ACTION_DOWN;
        if prevented && had_down && (starts_stream || !self.stream_open) {
            self.swallowing = true;
        }
        let consume = translation.swallow || (prevented && (had_down || has(PointerPhase::Wheel)));
        if starts_stream {
            self.stream_open = true;
        }
        let stream_end = raw.touch_stream && matches!(raw.action, ACTION_UP | ACTION_CANCEL);
        let hover = matches!(raw.action, ACTION_HOVER_MOVE | ACTION_HOVER_ENTER | ACTION_HOVER_EXIT);
        if stream_end || hover {
            // Hover means no contact: whatever stream there was is over.
            self.stream_open = false;
            self.swallowing = false;
        }
        if raw.captured && self.buttons == 0 && !raw.touch_stream {
            // Captured streams never reach views; a press ends when its
            // last button is released.
            self.swallowing = false;
        }
        consume
    }

    /// Held buttons after the last event (tests).
    #[cfg(test)]
    pub(crate) fn buttons(&self) -> u8 {
        self.buttons
    }

    /// Forget held buttons (the dispatcher synthesizes their `Up`s on
    /// focus loss; the next event must not re-diff against them).
    pub(crate) fn reset_buttons(&mut self) {
        self.buttons = 0;
        self.swallowing = false;
        self.stream_open = false;
        self.captured_pad = None;
    }
}

/// `Settings.Global.ANIMATOR_DURATION_SCALE` → reduce motion. The
/// developer/accessibility "Remove animations" switch sets the animator
/// scale to 0; any other value (1×, 0.5×, 10× …) still animates.
pub(crate) fn reduce_motion_from_animator_scale(scale: f32) -> bool {
    scale == 0.0
}

/// The device's primary pointer: a connected (non-virtual) mouse/touchpad
/// makes it `Fine` (CSS `pointer: fine` on a phone with a mouse); otherwise a
/// touch screen makes it `Coarse`; a device with neither (Android TV driven
/// by a D-pad remote) has `None`.
pub(crate) fn primary_pointer(has_mouse: bool, has_touchscreen: bool) -> PointerKind {
    if has_mouse {
        PointerKind::Fine
    } else if has_touchscreen {
        PointerKind::Coarse
    } else {
        PointerKind::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mouse(action: i32) -> RawMotion {
        RawMotion { touch_stream: matches!(action, ACTION_DOWN | ACTION_UP | ACTION_MOVE | ACTION_CANCEL), ..RawMotion::new(action, SOURCE_MOUSE, TOOL_TYPE_MOUSE) }
    }

    fn phases(t: &Translation) -> Vec<(PointerPhase, Option<MouseButton>)> {
        t.events.iter().map(|e| (e.phase, e.button)).collect()
    }

    #[test]
    fn button_state_maps_every_android_button() {
        assert_eq!(button_bits(BUTTON_PRIMARY), MouseButton::Primary.bit());
        assert_eq!(button_bits(BUTTON_SECONDARY), MouseButton::Secondary.bit());
        assert_eq!(button_bits(BUTTON_TERTIARY), MouseButton::Middle.bit());
        assert_eq!(button_bits(BUTTON_BACK), MouseButton::Back.bit());
        assert_eq!(button_bits(BUTTON_FORWARD), MouseButton::Forward.bit());
        assert_eq!(button_bits(BUTTON_STYLUS_PRIMARY), MouseButton::Secondary.bit());
        assert_eq!(button_bits(BUTTON_STYLUS_SECONDARY), MouseButton::Middle.bit());
        assert_eq!(button_bits(BUTTON_PRIMARY | BUTTON_SECONDARY), 3);
        assert_eq!(button_bits(0), 0);
    }

    #[test]
    fn wheel_sign_follows_web_delta_y() {
        // Android AXIS_VSCROLL +1 = wheel away from the user (scroll up)
        // → web deltaY negative.
        assert_eq!(wheel_px(0.0, 1.0), (0.0, -WHEEL_PX_PER_NOTCH));
        assert_eq!(wheel_px(0.0, -1.0), (0.0, WHEEL_PX_PER_NOTCH));
        // AXIS_HSCROLL +1 = scroll right = web deltaX positive.
        assert_eq!(wheel_px(1.0, 0.0), (WHEEL_PX_PER_NOTCH, 0.0));
        // Fractional (high-res wheel / touchpad) scales linearly.
        assert_eq!(wheel_px(0.0, -0.25), (0.0, 0.25 * WHEEL_PX_PER_NOTCH));
    }

    #[test]
    fn finger_touches_are_not_pointer_events() {
        assert!(!is_pointer_device(SOURCE_TOUCHSCREEN, TOOL_TYPE_FINGER));
        assert!(is_pointer_device(SOURCE_MOUSE, TOOL_TYPE_MOUSE));
        // Touchpad cursor: SOURCE_MOUSE with a finger tool (Android 13+).
        assert!(is_pointer_device(SOURCE_MOUSE, TOOL_TYPE_FINGER));
        assert!(is_pointer_device(SOURCE_TOUCHSCREEN | SOURCE_STYLUS, TOOL_TYPE_STYLUS));
        assert!(is_pointer_device(SOURCE_TOUCHSCREEN, TOOL_TYPE_ERASER));
        assert!(is_pointer_device(SOURCE_MOUSE_RELATIVE, TOOL_TYPE_MOUSE));
        let mut t = PointerTracker::default();
        let finger = RawMotion { touch_stream: true, ..RawMotion::new(ACTION_DOWN, SOURCE_TOUCHSCREEN, TOOL_TYPE_FINGER) };
        assert_eq!(t.translate(&finger), None);
    }

    #[test]
    fn positions_convert_px_to_dp_and_report_deltas() {
        assert_eq!(px_to_dp(300.0, 3.0), 100.0);
        assert_eq!(px_to_dp(42.0, 0.0), 42.0, "bad density falls back to 1");
        let mut t = PointerTracker::default();
        let first = t.translate(&RawMotion { x: 300.0, y: 150.0, density: 3.0, ..mouse(ACTION_HOVER_MOVE) }).unwrap();
        // First sample only seeds the position.
        assert!(first.events.is_empty());
        let second = t.translate(&RawMotion { x: 330.0, y: 120.0, density: 3.0, ..mouse(ACTION_HOVER_MOVE) }).unwrap();
        let mv = &second.events[0];
        assert_eq!(mv.phase, PointerPhase::Move);
        assert_eq!((mv.x, mv.y), (110.0, 40.0));
        assert_eq!((mv.dx, mv.dy), (10.0, -10.0));
    }

    #[test]
    fn regression_mouse_down_reported_on_touch_stream_down_not_button_press() {
        // The Down must arrive on ACTION_DOWN (the event View.onTouchEvent
        // turns into focus change / context menu), and the following
        // ACTION_BUTTON_PRESS must not report it twice.
        let mut t = PointerTracker::default();
        let down = t.translate(&RawMotion { button_state: BUTTON_SECONDARY, ..mouse(ACTION_DOWN) }).unwrap();
        assert_eq!(phases(&down), vec![(PointerPhase::Down, Some(MouseButton::Secondary))]);
        let press = t.translate(&RawMotion { button_state: BUTTON_SECONDARY, ..mouse(ACTION_BUTTON_PRESS) }).unwrap();
        assert!(press.events.is_empty());
        let release = t.translate(&mouse(ACTION_BUTTON_RELEASE)).unwrap();
        assert_eq!(phases(&release), vec![(PointerPhase::Up, Some(MouseButton::Secondary))]);
        let up = t.translate(&mouse(ACTION_UP)).unwrap();
        assert!(up.events.is_empty());
        assert_eq!(t.buttons(), 0);
    }

    #[test]
    fn second_button_during_drag_arrives_via_button_press() {
        let mut t = PointerTracker::default();
        t.translate(&RawMotion { button_state: BUTTON_PRIMARY, ..mouse(ACTION_DOWN) });
        let press = t
            .translate(&RawMotion { button_state: BUTTON_PRIMARY | BUTTON_TERTIARY, ..mouse(ACTION_BUTTON_PRESS) })
            .unwrap();
        assert_eq!(phases(&press), vec![(PointerPhase::Down, Some(MouseButton::Middle))]);
        assert_eq!(t.buttons(), 1 | 4);
    }

    #[test]
    fn stylus_contact_counts_as_primary() {
        let mut t = PointerTracker::default();
        let pen = |a| RawMotion { touch_stream: true, ..RawMotion::new(a, SOURCE_TOUCHSCREEN | SOURCE_STYLUS, TOOL_TYPE_STYLUS) };
        assert_eq!(phases(&t.translate(&pen(ACTION_DOWN)).unwrap()), vec![(PointerPhase::Down, Some(MouseButton::Primary))]);
        assert!(t.translate(&pen(ACTION_MOVE)).unwrap().events.is_empty());
        assert_eq!(phases(&t.translate(&pen(ACTION_UP)).unwrap()), vec![(PointerPhase::Up, Some(MouseButton::Primary))]);
    }

    #[test]
    fn touchpad_gesture_stream_does_not_press_primary() {
        // Android 14 touchpad two-finger swipes arrive as a SOURCE_MOUSE /
        // TOOL_TYPE_FINGER touch stream with buttonState 0 — not a click.
        let mut t = PointerTracker::default();
        let swipe = RawMotion { touch_stream: true, ..RawMotion::new(ACTION_DOWN, SOURCE_MOUSE, TOOL_TYPE_FINGER) };
        assert!(t.translate(&swipe).unwrap().events.is_empty());
        assert_eq!(t.buttons(), 0);
    }

    #[test]
    fn scroll_reports_wheel_with_modifiers_and_no_move() {
        let mut t = PointerTracker::default();
        t.translate(&RawMotion { x: 10.0, y: 10.0, ..mouse(ACTION_HOVER_MOVE) });
        let w = t
            .translate(&RawMotion { x: 50.0, y: 50.0, vscroll: -1.0, meta_state: 0x1000, ..mouse(ACTION_SCROLL) })
            .unwrap();
        assert_eq!(w.events.len(), 1);
        let e = &w.events[0];
        assert_eq!(e.phase, PointerPhase::Wheel);
        assert_eq!(e.wheel_dy, WHEEL_PX_PER_NOTCH);
        assert!(e.ctrl && !e.shift);
    }

    #[test]
    fn regression_prevented_secondary_down_swallows_whole_press_stream() {
        // The context menu can open from ACTION_DOWN (View.onTouchEvent) OR
        // from ACTION_BUTTON_PRESS (View.performContextClick); a prevented
        // Down must consume both and the rest of the stream.
        let mut t = PointerTracker::default();
        let raw_down = RawMotion { button_state: BUTTON_SECONDARY, ..mouse(ACTION_DOWN) };
        let tr = t.translate(&raw_down).unwrap();
        assert!(t.settle(&raw_down, &tr, true), "prevented Down consumed");
        for raw in [
            RawMotion { button_state: BUTTON_SECONDARY, ..mouse(ACTION_BUTTON_PRESS) },
            RawMotion { button_state: BUTTON_SECONDARY, x: 5.0, ..mouse(ACTION_MOVE) },
            mouse(ACTION_BUTTON_RELEASE),
            mouse(ACTION_UP),
        ] {
            let tr = t.translate(&raw).unwrap();
            assert!(t.settle(&raw, &tr, false), "action {} swallowed", raw.action);
        }
        // Next press, not prevented, passes through.
        let raw = RawMotion { button_state: BUTTON_PRIMARY, ..mouse(ACTION_DOWN) };
        let tr = t.translate(&raw).unwrap();
        assert!(!t.settle(&raw, &tr, false));
    }

    #[test]
    fn unprevented_events_pass_through() {
        let mut t = PointerTracker::default();
        let raw = RawMotion { vscroll: 1.0, ..mouse(ACTION_SCROLL) };
        let tr = t.translate(&raw).unwrap();
        assert!(!t.settle(&raw, &tr, false));
        assert!(t.settle(&raw, &tr, true), "prevented wheel consumed");
        // PreventDefault on a plain move means nothing: never consumed.
        let mv = RawMotion { x: 9.0, ..mouse(ACTION_HOVER_MOVE) };
        let tr = t.translate(&mv).unwrap();
        assert!(!t.settle(&mv, &tr, true));
    }

    #[test]
    fn regression_prevented_second_button_mid_drag_does_not_eat_the_streams_up() {
        // Views already own this stream (its DOWN was not prevented);
        // consuming its ACTION_UP would leave a view stuck pressed.
        let mut t = PointerTracker::default();
        let down = RawMotion { button_state: BUTTON_PRIMARY, ..mouse(ACTION_DOWN) };
        let tr = t.translate(&down).unwrap();
        assert!(!t.settle(&down, &tr, false));
        let press = RawMotion { button_state: BUTTON_PRIMARY | BUTTON_SECONDARY, ..mouse(ACTION_BUTTON_PRESS) };
        let tr = t.translate(&press).unwrap();
        assert!(t.settle(&press, &tr, true), "the prevented BUTTON_PRESS itself is consumed");
        let up = RawMotion { button_state: 0, ..mouse(ACTION_UP) };
        let tr = t.translate(&up).unwrap();
        assert!(!t.settle(&up, &tr, true), "the stream's UP still reaches the views");
    }

    #[test]
    fn captured_mouse_reports_raw_relative_motion_and_keeps_position() {
        let mut t = PointerTracker::default();
        t.translate(&RawMotion { x: 300.0, y: 300.0, density: 3.0, ..mouse(ACTION_HOVER_MOVE) });
        let raw = RawMotion {
            x: 7.0,
            y: -3.0,
            density: 3.0,
            captured: true,
            ..RawMotion::new(ACTION_MOVE, SOURCE_MOUSE_RELATIVE, TOOL_TYPE_MOUSE)
        };
        let tr = t.translate(&raw).unwrap();
        let e = &tr.events[0];
        assert_eq!(e.phase, PointerPhase::Move);
        assert_eq!((e.dx, e.dy), (7.0, -3.0), "raw counts, not density-scaled");
        assert_eq!((e.x, e.y), (100.0, 100.0), "position held at the lock point");
        // Captured buttons still diff.
        let b = t
            .translate(&RawMotion { button_state: BUTTON_PRIMARY, captured: true, ..RawMotion::new(ACTION_BUTTON_PRESS, SOURCE_MOUSE_RELATIVE, TOOL_TYPE_MOUSE) })
            .unwrap();
        assert_eq!(phases(&b), vec![(PointerPhase::Down, Some(MouseButton::Primary))]);
    }

    #[test]
    fn captured_touchpad_reports_deltas_between_absolute_samples() {
        let mut t = PointerTracker::default();
        let pad = |a, x, y| RawMotion { x, y, captured: true, touch_stream: true, ..RawMotion::new(a, SOURCE_TOUCHPAD, TOOL_TYPE_FINGER) };
        assert!(t.translate(&pad(ACTION_DOWN, 100.0, 100.0)).unwrap().events.is_empty());
        let mv = t.translate(&pad(ACTION_MOVE, 104.0, 98.0)).unwrap();
        assert_eq!((mv.events[0].dx, mv.events[0].dy), (4.0, -2.0));
        t.translate(&pad(ACTION_UP, 104.0, 98.0));
        // A new contact doesn't jump by the distance between contacts.
        assert!(t.translate(&pad(ACTION_DOWN, 10.0, 10.0)).unwrap().events.is_empty());
    }

    #[test]
    fn reset_forgets_held_buttons() {
        let mut t = PointerTracker::default();
        t.translate(&RawMotion { button_state: BUTTON_PRIMARY, ..mouse(ACTION_DOWN) });
        t.reset_buttons();
        // The press is re-reported once the next event shows it held.
        let tr = t.translate(&RawMotion { button_state: BUTTON_PRIMARY, ..mouse(ACTION_MOVE) }).unwrap();
        assert_eq!(phases(&tr), vec![(PointerPhase::Down, Some(MouseButton::Primary))]);
    }

    #[test]
    fn app_state_prefs_map() {
        assert!(reduce_motion_from_animator_scale(0.0));
        assert!(!reduce_motion_from_animator_scale(1.0));
        assert!(!reduce_motion_from_animator_scale(0.5));
        assert_eq!(primary_pointer(true, true), PointerKind::Fine);
        assert_eq!(primary_pointer(false, true), PointerKind::Coarse);
        assert_eq!(primary_pointer(false, false), PointerKind::None, "Android TV with a remote");
        assert_eq!(primary_pointer(true, false), PointerKind::Fine, "TV with a mouse");
    }
}
