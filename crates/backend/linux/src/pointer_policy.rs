//! Pure GDK → framework translation for the app-level pointer source
//! (`app_pointer.rs`) and the app-situation reports (`app_situation.rs`).
//!
//! Deliberately free of any GTK type: the crate body is
//! `cfg(target_os = "linux")`, so logic inside it could only be tested in
//! the Linux container. This file is ALSO compiled by
//! `tests/pointer_policy.rs` via `#[path]`, which runs on any host — the
//! reachable test for the translation rules (CLAUDE.md §8).

use runtime_shared::app_state::PointerKind;
use runtime_shared::pointer_input::MouseButton;

/// Logical pixels one wheel notch scrolls, for wheels GDK reports in
/// [`ScrollUnit::Wheel`] (a classic mouse wheel; `1.0` = one detent, high
/// resolution wheels report fractions). 40 px = three 13⅓ px lines, the
/// Windows default of "3 lines per notch" that the Win32 backend uses too,
/// so a notch scrolls the same distance on both desktops. Trackpads report
/// [`ScrollUnit::Surface`] — already logical pixels — and are NOT scaled.
pub const WHEEL_NOTCH_PX: f32 = runtime_shared::pointer_input::WHEEL_NOTCH_PX;

/// The framework button for a GDK button number (`gdk_button_event_get_button`).
///
/// GDK numbers buttons the X11 way on every backend (Wayland's
/// `BTN_LEFT`… are remapped by GDK): 1 left, 2 middle, 3 right, 8 back,
/// 9 forward. 4–7 are the legacy wheel buttons; GDK turns them into
/// scroll events, but an emulated press can still surface on X11 — they
/// are not buttons, so they map to `None` and are dropped.
pub fn gdk_button(button: u32) -> Option<MouseButton> {
    match button {
        1 => Some(MouseButton::Primary),
        2 => Some(MouseButton::Middle),
        3 => Some(MouseButton::Secondary),
        8 => Some(MouseButton::Back),
        9 => Some(MouseButton::Forward),
        _ => None,
    }
}

/// GDK's scroll-event shapes, without GDK types.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum Scroll {
    /// A discrete step (`GDK_SCROLL_UP/DOWN/LEFT/RIGHT`): one notch.
    Up,
    Down,
    Left,
    Right,
    /// `GDK_SCROLL_SMOOTH` with its deltas. `surface_units` is
    /// `gdk_scroll_event_get_unit() == GDK_SCROLL_UNIT_SURFACE` (trackpads:
    /// the deltas are already logical pixels); otherwise the unit is
    /// `WHEEL` and `1.0` is one notch.
    Smooth { dx: f64, dy: f64, surface_units: bool },
}

/// Wheel amounts in logical pixels with the Web `WheelEvent.deltaY` sign:
/// positive `dy` = wheel toward the user = content scrolls down.
///
/// GDK already uses that sign (`GDK_SCROLL_DOWN` and a positive smooth
/// `dy` both mean "scroll the content down"), so only units change.
pub fn scroll_px(s: Scroll) -> (f32, f32) {
    match s {
        Scroll::Up => (0.0, -WHEEL_NOTCH_PX),
        Scroll::Down => (0.0, WHEEL_NOTCH_PX),
        Scroll::Left => (-WHEEL_NOTCH_PX, 0.0),
        Scroll::Right => (WHEEL_NOTCH_PX, 0.0),
        Scroll::Smooth { dx, dy, surface_units: true } => (dx as f32, dy as f32),
        Scroll::Smooth { dx, dy, surface_units: false } => {
            (dx as f32 * WHEEL_NOTCH_PX, dy as f32 * WHEEL_NOTCH_PX)
        }
    }
}

/// Movement since the previous pointer event. The first event after the
/// source is installed (or after the pointer re-enters) has no previous
/// position and reports zero movement rather than a jump from the origin.
pub fn motion_delta(prev: Option<(f32, f32)>, now: (f32, f32)) -> (f32, f32) {
    match prev {
        Some((px, py)) => (now.0 - px, now.1 - py),
        None => (0.0, 0.0),
    }
}

/// The point the pointer is held at while locked: the centre of a
/// `width × height` surface, in surface coordinates (integer, because the
/// X server warps to whole device pixels).
pub fn lock_center(width: i32, height: i32) -> (i32, i32) {
    (width.max(0) / 2, height.max(0) / 2)
}

/// `gtk-enable-animations` → the reduce-motion fact. GTK's setting is the
/// desktop's "animations" switch (GNOME Settings → Accessibility → Reduce
/// Animation writes `org.gnome.desktop.interface enable-animations`, which
/// GTK mirrors here), so animations OFF means the user asked for less
/// motion.
pub fn reduce_motion(enable_animations: bool) -> bool {
    !enable_animations
}

/// The primary pointer from the seat's capabilities
/// (`GDK_SEAT_CAPABILITY_POINTER` / `_TOUCH`): any pointing device (mouse,
/// trackpad, pen) makes it `Fine` — a touch-screen laptop with a trackpad
/// is a fine-pointer machine, like CSS `(pointer: fine)` reports there;
/// touch alone is `Coarse`; neither is `None`.
pub fn pointer_kind(has_pointer: bool, has_touch: bool) -> PointerKind {
    if has_pointer {
        PointerKind::Fine
    } else if has_touch {
        PointerKind::Coarse
    } else {
        PointerKind::None
    }
}

/// Movement bookkeeping while the pointer is locked (X11 warp lock, see
/// `x11_lock.rs`).
///
/// After each reported motion the pointer is warped back to the lock
/// centre, and the warp itself produces a motion event at the centre (its
/// "echo"). Measuring every motion against the centre would double-count:
/// motion the user makes BEFORE the server has processed the warp still
/// arrives relative to the un-warped position. So each motion is measured
/// against the PREVIOUS event's position, and an echo (an event exactly on
/// the centre while a warp is outstanding) just moves the reference back
/// to the centre without reporting anything.
#[derive(Clone, Debug, PartialEq)]
pub struct LockTracker {
    center: (f64, f64),
    reference: (f64, f64),
    pending_warps: u32,
}

impl LockTracker {
    /// A lock that just warped the pointer from `from` (its last known
    /// position, if any) to `center`.
    pub fn new(center: (i32, i32), from: Option<(f64, f64)>) -> Self {
        let center = (center.0 as f64, center.1 as f64);
        match from {
            Some(p) if p != center => LockTracker { center, reference: p, pending_warps: 1 },
            _ => LockTracker { center, reference: center, pending_warps: 0 },
        }
    }

    /// One motion event at surface position `pos`. Returns the movement
    /// to report (if any) and whether to warp the pointer back to the
    /// centre now.
    pub fn on_motion(&mut self, pos: (f64, f64)) -> (Option<(f32, f32)>, bool) {
        if pos == self.center && self.pending_warps > 0 {
            self.pending_warps -= 1;
            self.reference = self.center;
            return (None, false);
        }
        let d = ((pos.0 - self.reference.0) as f32, (pos.1 - self.reference.1) as f32);
        self.reference = pos;
        if d == (0.0, 0.0) {
            return (None, false);
        }
        let warp = pos != self.center;
        if warp {
            self.pending_warps += 1;
        }
        (Some(d), warp)
    }
}
