//! Pure translation behind the macOS app-level pointer source
//! (`imp/pointer.rs`): `NSEvent` scalars → [`AppPointerEvent`], the
//! swallow decision for a `PreventDefault`, the held-button reconcile that
//! catches releases AppKit's tracking loops eat, and the app-visibility
//! reading. Un-gated so the tests run from any host; the `NSEvent` monitor,
//! notification observers and cursor calls that feed it are macos-only.
//!
//! ## Conventions this pins
//!
//! - **Buttons**: left/right events by type; other-mouse events by
//!   `-[NSEvent buttonNumber]` 2/3/4 → Middle / Back / Forward — the same
//!   numbering web's `MouseEvent.button` uses (3 = back, 4 = forward).
//! - **Wheel sign**: AppKit's `scrollingDeltaY` is positive when the content
//!   moves DOWN (the view scrolls toward its top); web's `WheelEvent.deltaY`
//!   is positive when the view scrolls toward its bottom. So both axes are
//!   negated. "Natural scrolling" is NOT undone: the system has already
//!   applied the user's direction preference to `scrollingDelta*`
//!   (`isDirectionInvertedFromDevice` only reports that it did), and
//!   browsers on macOS report the same post-preference direction — so a
//!   gesture that scrolls a web page down yields a positive `wheel_dy` here
//!   too.
//! - **Wheel units**: with `hasPreciseScrollingDeltas` (trackpad, Magic
//!   Mouse) the deltas are points = logical px. Otherwise (a notched wheel)
//!   they are LINES; [`WHEEL_PX_PER_LINE`] converts.

use runtime_shared::pointer_input::{AppPointerEvent, MouseButton, PointerOutcome, PointerPhase};

/// Pixels per wheel line for a non-precise (notched) wheel: Chromium's
/// `kScrollbarPixelsPerCocoaTick` (40), i.e. what one notch scrolls a web
/// page by in Chrome on macOS — so the same mouse produces the same
/// `wheel_dy` in the web build and the native build.
pub(crate) const WHEEL_PX_PER_LINE: f32 = runtime_shared::pointer_input::WHEEL_NOTCH_PX;

// `NSEventType` values the pointer monitor watches.
pub(crate) const T_LEFT_DOWN: usize = 1;
pub(crate) const T_LEFT_UP: usize = 2;
pub(crate) const T_RIGHT_DOWN: usize = 3;
pub(crate) const T_RIGHT_UP: usize = 4;
pub(crate) const T_MOVED: usize = 5;
pub(crate) const T_LEFT_DRAGGED: usize = 6;
pub(crate) const T_RIGHT_DRAGGED: usize = 7;
pub(crate) const T_KEY_DOWN: usize = 10;
pub(crate) const T_SCROLL_WHEEL: usize = 22;
pub(crate) const T_OTHER_DOWN: usize = 25;
pub(crate) const T_OTHER_UP: usize = 26;
pub(crate) const T_OTHER_DRAGGED: usize = 27;

/// `NSEventMask` (`1 << NSEventType`) for every event the monitor watches.
/// `KeyDown` is included only so Esc can release a held lock (web parity:
/// the browser drops pointer lock on Esc) — key events are always returned
/// unchanged.
pub(crate) const MONITOR_MASK: u64 = (1 << T_LEFT_DOWN)
    | (1 << T_LEFT_UP)
    | (1 << T_RIGHT_DOWN)
    | (1 << T_RIGHT_UP)
    | (1 << T_MOVED)
    | (1 << T_LEFT_DRAGGED)
    | (1 << T_RIGHT_DRAGGED)
    | (1 << T_KEY_DOWN)
    | (1 << T_SCROLL_WHEEL)
    | (1 << T_OTHER_DOWN)
    | (1 << T_OTHER_UP)
    | (1 << T_OTHER_DRAGGED);

/// `kVK_Escape`.
pub(crate) const KEY_CODE_ESCAPE: u16 = 0x35;

// Device-independent `NSEventModifierFlags` bits.
const FLAG_SHIFT: usize = 1 << 17;
const FLAG_CONTROL: usize = 1 << 18;
const FLAG_OPTION: usize = 1 << 19;
const FLAG_COMMAND: usize = 1 << 20;

/// `NSApplicationOcclusionStateVisible`.
pub(crate) const OCCLUSION_VISIBLE: usize = 1 << 1;

/// The scalars read off one `NSEvent` (see `imp/pointer.rs::read_raw`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct RawPointer {
    pub event_type: usize,
    pub button_number: isize,
    /// Position relative to the app root, top-left origin, logical px.
    pub x: f32,
    pub y: f32,
    /// `deltaX` / `deltaY` (movement; y grows downward, like the screen).
    pub delta_x: f32,
    pub delta_y: f32,
    /// `scrollingDeltaX` / `scrollingDeltaY` (wheel only).
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub precise: bool,
    pub flags: usize,
}

impl RawPointer {
    /// A blank raw event of `event_type` at `(x, y)` (tests + read_raw).
    pub(crate) fn new(event_type: usize, x: f32, y: f32) -> Self {
        RawPointer {
            event_type,
            button_number: 0,
            x,
            y,
            delta_x: 0.0,
            delta_y: 0.0,
            scroll_x: 0.0,
            scroll_y: 0.0,
            precise: true,
            flags: 0,
        }
    }
}

/// `buttonNumber` → [`MouseButton`]. Buttons past 4 have no web equivalent
/// and are not delivered.
pub(crate) fn button_from_number(n: isize) -> Option<MouseButton> {
    match n {
        0 => Some(MouseButton::Primary),
        1 => Some(MouseButton::Secondary),
        2 => Some(MouseButton::Middle),
        3 => Some(MouseButton::Back),
        4 => Some(MouseButton::Forward),
        _ => None,
    }
}

/// The button a down/up is about. Left/right events name their button by
/// TYPE — `buttonNumber` is only meaningful for the `OtherMouse*` types
/// (AppKit-synthesized right-mouse events carry `buttonNumber` 0) — and
/// other-mouse events by [`button_from_number`].
pub(crate) fn button_of(event_type: usize, button_number: isize) -> Option<MouseButton> {
    match event_type {
        T_LEFT_DOWN | T_LEFT_UP => Some(MouseButton::Primary),
        T_RIGHT_DOWN | T_RIGHT_UP => Some(MouseButton::Secondary),
        _ => button_from_number(button_number),
    }
}

/// The phase an `NSEventType` maps to (`None` for types the pointer source
/// does not translate — the Esc key-down, anything else).
pub(crate) fn phase_of(event_type: usize) -> Option<PointerPhase> {
    match event_type {
        T_LEFT_DOWN | T_RIGHT_DOWN | T_OTHER_DOWN => Some(PointerPhase::Down),
        T_LEFT_UP | T_RIGHT_UP | T_OTHER_UP => Some(PointerPhase::Up),
        T_MOVED | T_LEFT_DRAGGED | T_RIGHT_DRAGGED | T_OTHER_DRAGGED => Some(PointerPhase::Move),
        T_SCROLL_WHEEL => Some(PointerPhase::Wheel),
        _ => None,
    }
}

/// Wheel deltas in web convention (see the module docs): negated, and
/// line-mode deltas scaled by [`WHEEL_PX_PER_LINE`].
pub(crate) fn wheel_px(scroll_x: f32, scroll_y: f32, precise: bool) -> (f32, f32) {
    let scale = if precise { 1.0 } else { WHEEL_PX_PER_LINE };
    (-scroll_x * scale, -scroll_y * scale)
}

/// `(shift, ctrl, alt, meta)` from `modifierFlags`.
pub(crate) fn modifiers(flags: usize) -> (bool, bool, bool, bool) {
    (
        flags & FLAG_SHIFT != 0,
        flags & FLAG_CONTROL != 0,
        flags & FLAG_OPTION != 0,
        flags & FLAG_COMMAND != 0,
    )
}

/// The full translation. `None` when the event is not a pointer event the
/// sink takes (an unknown type, a button past Forward).
pub(crate) fn translate(raw: &RawPointer) -> Option<AppPointerEvent> {
    let phase = phase_of(raw.event_type)?;
    let (shift, ctrl, alt, meta) = modifiers(raw.flags);
    let mut ev = AppPointerEvent { shift, ctrl, alt, meta, ..AppPointerEvent::new(phase, raw.x, raw.y) };
    match phase {
        PointerPhase::Down | PointerPhase::Up => {
            ev.button = Some(button_of(raw.event_type, raw.button_number)?);
        }
        PointerPhase::Move => {
            ev.dx = raw.delta_x;
            ev.dy = raw.delta_y;
        }
        PointerPhase::Wheel => {
            let (wx, wy) = wheel_px(raw.scroll_x, raw.scroll_y, raw.precise);
            ev.wheel_dx = wx;
            ev.wheel_dy = wy;
        }
    }
    Some(ev)
}

/// Whether the monitor returns `nil` (swallows the event) for this outcome.
/// Only a down and a wheel honor `PreventDefault`: swallowing the down keeps
/// AppKit from running `mouseDown:` / `rightMouseDown:` — no focus change,
/// no text selection, and for a secondary (or ctrl-) press no
/// `menuForEvent:` context menu; swallowing the wheel stops the scroll.
/// Moves and releases always pass through — eating a release would leave an
/// AppKit control that DID see its press stuck mid-track.
pub(crate) fn should_swallow(phase: PointerPhase, outcome: PointerOutcome) -> bool {
    outcome == PointerOutcome::PreventDefault && matches!(phase, PointerPhase::Down | PointerPhase::Wheel)
}

/// Buttons in `held` ([`MouseButton::bit`]s we delivered a `Down` for and no
/// `Up` yet) that the hardware no longer reports pressed
/// (`+[NSEvent pressedMouseButtons]`: bit n = `buttonNumber` n).
///
/// AppKit only runs local monitors for events dispatched through
/// `-[NSApplication sendEvent:]`; a control's nested tracking loop (NSButton,
/// NSSlider, text selection, window drag) dequeues the matching mouse-up
/// itself and the monitor never sees it. Without this reconcile the
/// dispatcher would believe the button held forever.
pub(crate) fn released_but_held(held: u8, pressed_mask: usize) -> Vec<MouseButton> {
    [MouseButton::Primary, MouseButton::Secondary, MouseButton::Middle, MouseButton::Back, MouseButton::Forward]
        .into_iter()
        .enumerate()
        .filter(|(n, b)| held & b.bit() != 0 && pressed_mask & (1 << n) == 0)
        .map(|(_, b)| b)
        .collect()
}

/// The app is on screen: not hidden (Cmd-H) and its occlusion state has the
/// visible bit (false when every window is minimized, fully covered, on
/// another Space, or the screen is locked/asleep).
pub(crate) fn app_visible(is_hidden: bool, occlusion_state: usize) -> bool {
    !is_hidden && occlusion_state & OCCLUSION_VISIBLE != 0
}

/// Cocoa global coordinates (bottom-left origin of the primary screen) →
/// Quartz global display coordinates (top-left origin), which
/// `CGWarpMouseCursorPosition` takes.
pub(crate) fn cocoa_to_quartz(x: f64, y: f64, primary_screen_height: f64) -> (f64, f64) {
    (x, primary_screen_height - y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_numbers_match_web_button_numbering() {
        assert_eq!(button_from_number(0), Some(MouseButton::Primary));
        assert_eq!(button_from_number(1), Some(MouseButton::Secondary));
        assert_eq!(button_from_number(2), Some(MouseButton::Middle));
        assert_eq!(button_from_number(3), Some(MouseButton::Back));
        assert_eq!(button_from_number(4), Some(MouseButton::Forward));
        assert_eq!(button_from_number(5), None);
        assert_eq!(button_from_number(-1), None);
    }

    /// REGRESSION: a right-mouse event whose `buttonNumber` reads 0 (as
    /// `+mouseEventWithType:` synthesizes it) was reported as Primary.
    #[test]
    fn regression_left_right_named_by_type_not_button_number() {
        assert_eq!(button_of(T_RIGHT_DOWN, 0), Some(MouseButton::Secondary));
        assert_eq!(button_of(T_RIGHT_UP, 0), Some(MouseButton::Secondary));
        assert_eq!(button_of(T_LEFT_DOWN, 1), Some(MouseButton::Primary));
        assert_eq!(button_of(T_OTHER_DOWN, 2), Some(MouseButton::Middle));
        assert_eq!(button_of(T_OTHER_UP, 3), Some(MouseButton::Back));
        assert_eq!(button_of(T_OTHER_DOWN, 4), Some(MouseButton::Forward));
    }

    #[test]
    fn every_watched_type_has_a_phase() {
        for (t, p) in [
            (T_LEFT_DOWN, PointerPhase::Down),
            (T_RIGHT_DOWN, PointerPhase::Down),
            (T_OTHER_DOWN, PointerPhase::Down),
            (T_LEFT_UP, PointerPhase::Up),
            (T_RIGHT_UP, PointerPhase::Up),
            (T_OTHER_UP, PointerPhase::Up),
            (T_MOVED, PointerPhase::Move),
            (T_LEFT_DRAGGED, PointerPhase::Move),
            (T_RIGHT_DRAGGED, PointerPhase::Move),
            (T_OTHER_DRAGGED, PointerPhase::Move),
            (T_SCROLL_WHEEL, PointerPhase::Wheel),
        ] {
            assert_eq!(phase_of(t), Some(p), "type {t}");
            assert!(MONITOR_MASK & (1 << t) != 0, "type {t} in the monitor mask");
        }
        assert_eq!(phase_of(T_KEY_DOWN), None, "Esc rides the mask but is not a pointer event");
    }

    /// REGRESSION (web parity): a notched wheel rolled toward the user must
    /// read as positive `wheel_dy` in px, like web `deltaY`. AppKit reports
    /// that as a NEGATIVE `scrollingDeltaY` in lines.
    #[test]
    fn regression_wheel_toward_user_is_positive_px_like_web() {
        assert_eq!(wheel_px(0.0, -1.0, false), (-0.0, WHEEL_PX_PER_LINE));
        assert_eq!(wheel_px(0.0, 2.0, false), (-0.0, -2.0 * WHEEL_PX_PER_LINE));
        // Precise (trackpad) deltas are already points.
        assert_eq!(wheel_px(3.5, -12.0, true), (-3.5, 12.0));
    }

    #[test]
    fn translate_fills_button_motion_wheel_and_modifiers() {
        let down = translate(&RawPointer {
            button_number: 1,
            flags: FLAG_CONTROL | FLAG_SHIFT,
            ..RawPointer::new(T_RIGHT_DOWN, 10.0, 20.0)
        })
        .unwrap();
        assert_eq!((down.phase, down.button, down.x, down.y), (PointerPhase::Down, Some(MouseButton::Secondary), 10.0, 20.0));
        assert!(down.ctrl && down.shift && !down.alt && !down.meta);

        let moved = translate(&RawPointer { delta_x: 3.0, delta_y: -4.0, ..RawPointer::new(T_LEFT_DRAGGED, 1.0, 2.0) }).unwrap();
        assert_eq!((moved.phase, moved.button, moved.dx, moved.dy), (PointerPhase::Move, None, 3.0, -4.0));

        let wheel = translate(&RawPointer {
            scroll_y: -5.0,
            precise: true,
            flags: FLAG_COMMAND | FLAG_OPTION,
            ..RawPointer::new(T_SCROLL_WHEEL, 0.0, 0.0)
        })
        .unwrap();
        assert_eq!((wheel.wheel_dx, wheel.wheel_dy), (-0.0, 5.0));
        assert!(wheel.meta && wheel.alt);

        // A sixth button has no web name — dropped, not mislabeled.
        assert!(translate(&RawPointer { button_number: 7, ..RawPointer::new(T_OTHER_DOWN, 0.0, 0.0) }).is_none());
        assert!(translate(&RawPointer::new(T_KEY_DOWN, 0.0, 0.0)).is_none());
    }

    /// REGRESSION (context menu during play): a claimed secondary down must
    /// be swallowed so AppKit never reaches `rightMouseDown:` →
    /// `menuForEvent:`; releases and moves are never swallowed.
    #[test]
    fn regression_prevent_default_swallows_down_and_wheel_only() {
        assert!(should_swallow(PointerPhase::Down, PointerOutcome::PreventDefault));
        assert!(should_swallow(PointerPhase::Wheel, PointerOutcome::PreventDefault));
        assert!(!should_swallow(PointerPhase::Up, PointerOutcome::PreventDefault));
        assert!(!should_swallow(PointerPhase::Move, PointerOutcome::PreventDefault));
        assert!(!should_swallow(PointerPhase::Down, PointerOutcome::Default));
    }

    /// REGRESSION (stuck button): a press on an NSButton/NSSlider runs a
    /// tracking loop that eats the mouse-up before any monitor sees it.
    #[test]
    fn regression_release_eaten_by_tracking_loop_is_detected() {
        let held = MouseButton::Primary.bit() | MouseButton::Middle.bit();
        // Hardware: only the middle button (buttonNumber 2) still down.
        assert_eq!(released_but_held(held, 1 << 2), vec![MouseButton::Primary]);
        assert_eq!(released_but_held(held, 0), vec![MouseButton::Primary, MouseButton::Middle]);
        assert!(released_but_held(held, 0b101).is_empty());
        assert!(released_but_held(0, 0).is_empty());
    }

    #[test]
    fn visibility_needs_unhidden_and_visible_occlusion() {
        assert!(app_visible(false, OCCLUSION_VISIBLE));
        assert!(!app_visible(true, OCCLUSION_VISIBLE));
        assert!(!app_visible(false, 0));
    }

    #[test]
    fn cocoa_to_quartz_flips_about_the_primary_screen() {
        assert_eq!(cocoa_to_quartz(100.0, 200.0, 1000.0), (100.0, 800.0));
    }
}
