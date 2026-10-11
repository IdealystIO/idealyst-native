//! App-level pointer source, pointer lock and cursor hiding for the macOS
//! backend. Drives `MacosBackend::{set_pointer_sink_impl,
//! set_pointer_lock_impl, set_cursor_hidden_impl}`; the pure translation
//! lives in `crate::pointer_policy`.
//!
//! - **Source**: one `NSEvent addLocalMonitorForEventsMatchingMask:` monitor
//!   for left/right/other down + up + dragged, mouse-moved and scroll-wheel.
//!   A local monitor sees every mouse event the app dispatches through
//!   `-[NSApplication sendEvent:]`, whatever view is under the pointer —
//!   including a focused text field. Returning `nil` swallows the event:
//!   that is how `PreventDefault` on a down skips `mouseDown:` (focus,
//!   selection) and, for a secondary / ctrl press, `rightMouseDown:` →
//!   `menuForEvent:` (the context menu); on a wheel it skips the scroll.
//! - **Mouse-moved** events are only generated for a window whose
//!   `acceptsMouseMovedEvents` is set; the source sets it on the host window
//!   while installed and restores the previous value on teardown.
//! - **Eaten releases**: a control's own tracking loop (NSButton, NSSlider,
//!   text selection, title-bar drag) dequeues its mouse-up without going
//!   through `sendEvent:`, so the monitor never sees it. While a button we
//!   reported down is held, a 100 ms poll compares against
//!   `+[NSEvent pressedMouseButtons]` and synthesizes the missing `Up`
//!   (only when no matching up is still waiting in the queue).
//! - **Lock**: `CGAssociateMouseAndMouseCursorPosition(false)` freezes the
//!   cursor (it cannot leave the window); mouse-moved / dragged events keep
//!   arriving with device `deltaX`/`deltaY`, the position stays put. The
//!   cursor is hidden with a balanced `+[NSCursor hide]`. Granted
//!   synchronously, and only while the app is active — disassociation is
//!   session-wide, so locking from the background would freeze the user's
//!   cursor in another app. Dropped (reported `lock_changed(false)`) when
//!   the app resigns active, a window resigns key, or Esc is pressed (the
//!   browser's built-in escape hatch, kept identical here).
//! - **Cursor hiding** is process-global in AppKit (`+[NSCursor hide]` is
//!   counted), so its state lives in a thread-local [`CURSOR`] rather than
//!   on the source: a `set_cursor_hidden(false)` can arrive after the
//!   dispatcher already removed the source. Teardown always restores the
//!   cursor and releases the lock.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{class, msg_send};
use objc2_foundation::{CGPoint, CGRect, NSObject};
use runtime_shared::pointer_input::{AppPointerEvent, PointerPhase, PointerSink};

use super::MacosBackend;
use crate::pointer_policy::{self as policy, RawPointer};

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGAssociateMouseAndMouseCursorPosition(connected: u32) -> i32;
    fn CGWarpMouseCursorPosition(new_position: CGPoint) -> i32;
}

/// How often the eaten-release poll runs while a button is held.
const RELEASE_POLL_MS: i32 = 100;

/// `NSEventMask` for the three mouse-up types (queue peek in the poll).
const MASK_MOUSE_UPS: u64 =
    (1 << policy::T_LEFT_UP) | (1 << policy::T_RIGHT_UP) | (1 << policy::T_OTHER_UP);

#[derive(Default)]
struct CursorState {
    /// Pointer lock held (mouse disassociated from the cursor).
    locked: bool,
    /// The app asked for the cursor hidden (`set_cursor_hidden(true)`).
    user_hidden: bool,
    /// Whether WE currently hold one `+[NSCursor hide]`. `hide`/`unhide`
    /// are counted, so every hide must be paired with exactly one unhide.
    applied_hidden: bool,
}

thread_local! {
    static CURSOR: RefCell<CursorState> = RefCell::new(CursorState::default());
    /// Test seam: skip the "app must be active" gate. A `cargo test` binary
    /// is a background process the OS may refuse to activate, which would
    /// leave the grant / platform-drop paths unexercised. Never set outside
    /// the AppKit integration test.
    static SKIP_ACTIVE_GATE: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn skip_active_gate_for_tests(skip: bool) {
    SKIP_ACTIVE_GATE.with(|s| s.set(skip));
}

/// Hide or show the cursor so `applied_hidden == locked || user_hidden`,
/// issuing at most one balanced `hide`/`unhide`.
fn apply_cursor() {
    let change = CURSOR.with(|c| {
        let mut c = c.borrow_mut();
        let want = c.locked || c.user_hidden;
        (want != c.applied_hidden).then(|| {
            c.applied_hidden = want;
            want
        })
    });
    match change {
        Some(true) => unsafe {
            let _: () = msg_send![class!(NSCursor), hide];
        },
        Some(false) => unsafe {
            let _: () = msg_send![class!(NSCursor), unhide];
        },
        None => {}
    }
}

pub(crate) fn is_locked() -> bool {
    CURSOR.with(|c| c.borrow().locked)
}

/// State shared with the monitor block, the observers and the release poll.
struct Shared {
    /// Callers CLONE the sink out and drop the borrow before calling it: a
    /// listener may add/remove listeners, which re-enters
    /// `set_pointer_sink` on the backend.
    sink: RefCell<Option<PointerSink>>,
    /// The host root (a top-left-origin `FlippedView`) positions are
    /// reported relative to.
    host_root: Option<Retained<objc2_app_kit::NSView>>,
    /// `MouseButton::bit`s we delivered a `Down` for and no `Up` yet.
    held: Cell<u8>,
    /// A release poll is scheduled.
    poll_armed: Cell<bool>,
    /// Cleared on teardown so a queued poll / observer turn does nothing.
    alive: Cell<bool>,
}

impl Shared {
    fn sink(&self) -> Option<PointerSink> {
        self.sink.borrow().clone()
    }
}

/// The installed app-level pointer source.
pub(crate) struct AppPointer {
    shared: Rc<Shared>,
    monitor: Retained<NSObject>,
    observers: Vec<Retained<NSObject>>,
    /// The host window and its `acceptsMouseMovedEvents` before install.
    restore_moved: Option<(Retained<AnyObject>, bool)>,
}

impl AppPointer {
    fn teardown(self) {
        self.shared.alive.set(false);
        self.shared.sink.borrow_mut().take();
        unsafe {
            let _: () = msg_send![class!(NSEvent), removeMonitor: &*self.monitor];
        }
        for observer in &self.observers {
            super::callbacks::remove_observer(observer);
        }
        if let Some((window, prev)) = &self.restore_moved {
            let _: () = unsafe { msg_send![&**window, setAcceptsMouseMovedEvents: *prev] };
        }
        // Never leave the user's cursor frozen or hidden behind a source
        // that no longer exists. No report: the sink is gone.
        release_lock_quietly();
        CURSOR.with(|c| c.borrow_mut().user_hidden = false);
        apply_cursor();
    }
}

/// The host root's window, if mounted.
unsafe fn host_window(root: &Option<Retained<objc2_app_kit::NSView>>) -> Option<Retained<AnyObject>> {
    let root = root.as_ref()?;
    let win: *mut AnyObject = msg_send![&**root, window];
    Retained::retain(win)
}

/// `locationInWindow` → host-root coordinates, top-left origin, points
/// (= logical px). An event from another of the app's windows (or a
/// window-less synthetic event, whose location is already in screen
/// coordinates) goes through screen space.
unsafe fn local_point(shared: &Shared, event: *mut NSObject) -> (f32, f32) {
    let loc: CGPoint = msg_send![event, locationInWindow];
    let Some(root) = shared.host_root.as_ref() else {
        return (loc.x as f32, loc.y as f32);
    };
    let root_win: *mut AnyObject = msg_send![&**root, window];
    if root_win.is_null() {
        return (loc.x as f32, loc.y as f32);
    }
    let ev_win: *mut AnyObject = msg_send![event, window];
    let in_root_win = if ev_win == root_win {
        loc
    } else {
        let screen: CGPoint = if ev_win.is_null() { loc } else { msg_send![ev_win, convertPointToScreen: loc] };
        msg_send![root_win, convertPointFromScreen: screen]
    };
    let nil: *mut AnyObject = std::ptr::null_mut();
    let p: CGPoint = msg_send![&**root, convertPoint: in_root_win, fromView: nil];
    // `convertPoint:fromView:` already lands in the root's own coordinate
    // system; the host root is a `FlippedView` (top-left origin). Flip only
    // for a host that installed a non-flipped root.
    let flipped: bool = msg_send![&**root, isFlipped];
    let y = if flipped {
        p.y
    } else {
        let b: CGRect = msg_send![&**root, bounds];
        b.size.height - p.y
    };
    (p.x as f32, y as f32)
}

/// Read the scalars `pointer_policy::translate` needs. Only the selectors
/// valid for the event's type are sent — `scrollingDeltaY` on a key event
/// (or `deltaX` on a scroll) raises an Objective-C exception.
unsafe fn read_raw(shared: &Shared, event: *mut NSObject, event_type: usize) -> Option<RawPointer> {
    let phase = policy::phase_of(event_type)?;
    let (x, y) = local_point(shared, event);
    let mut raw = RawPointer::new(event_type, x, y);
    raw.flags = msg_send![event, modifierFlags];
    match phase {
        PointerPhase::Down | PointerPhase::Up => {
            raw.button_number = msg_send![event, buttonNumber];
        }
        PointerPhase::Move => {
            let dx: f64 = msg_send![event, deltaX];
            let dy: f64 = msg_send![event, deltaY];
            raw.delta_x = dx as f32;
            raw.delta_y = dy as f32;
        }
        PointerPhase::Wheel => {
            let sx: f64 = msg_send![event, scrollingDeltaX];
            let sy: f64 = msg_send![event, scrollingDeltaY];
            raw.scroll_x = sx as f32;
            raw.scroll_y = sy as f32;
            raw.precise = msg_send![event, hasPreciseScrollingDeltas];
        }
    }
    Some(raw)
}

/// Deliver one event, keeping `held` in step. Returns the outcome.
fn deliver(shared: &Rc<Shared>, ev: &AppPointerEvent) -> runtime_shared::pointer_input::PointerOutcome {
    if let Some(b) = ev.button {
        match ev.phase {
            PointerPhase::Down => shared.held.set(shared.held.get() | b.bit()),
            PointerPhase::Up => shared.held.set(shared.held.get() & !b.bit()),
            _ => {}
        }
    }
    let outcome = match shared.sink() {
        Some(sink) => sink.event(ev),
        None => runtime_shared::pointer_input::PointerOutcome::Default,
    };
    if ev.phase == PointerPhase::Down {
        arm_release_poll(shared);
    }
    outcome
}

fn arm_release_poll(shared: &Rc<Shared>) {
    // Without a scheduler `after_ms` runs inline, and a re-arming tick
    // would recurse for as long as the button is physically held. Every
    // real boot (and the AppKit test) installs the Apple scheduler.
    if !runtime_shared::scheduling::is_scheduler_installed() {
        return;
    }
    if shared.held.get() == 0 || shared.poll_armed.replace(true) {
        return;
    }
    let weak = Rc::downgrade(shared);
    // Detached: the tick checks `alive` (and the weak upgrade) itself, so a
    // tick queued across a teardown does nothing.
    runtime_shared::scheduling::after_ms_detached(RELEASE_POLL_MS, move || {
        backend_apple_core::crash::abort_on_panic("app pointer release poll", || {
            let Some(shared) = weak.upgrade() else {
                return;
            };
            shared.poll_armed.set(false);
            if !shared.alive.get() {
                return;
            }
            reconcile_released(&shared);
            arm_release_poll(&shared);
        });
    });
}

/// Synthesize the `Up` a tracking loop ate (see the module docs).
fn reconcile_released(shared: &Rc<Shared>) {
    let pressed: usize = unsafe { msg_send![class!(NSEvent), pressedMouseButtons] };
    let stale = policy::released_but_held(shared.held.get(), pressed);
    if stale.is_empty() {
        return;
    }
    // A matching up still in the queue will arrive through the monitor —
    // don't deliver it twice.
    let pending: *mut AnyObject = unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        let mode = objc2_foundation::NSString::from_str("kCFRunLoopDefaultMode");
        let nil: *mut AnyObject = std::ptr::null_mut();
        msg_send![app, nextEventMatchingMask: MASK_MOUSE_UPS, untilDate: nil, inMode: &*mode, dequeue: false]
    };
    if !pending.is_null() {
        return;
    }
    let pos = runtime_shared::pointer_input::position();
    for b in stale {
        let up = AppPointerEvent { button: Some(b), ..AppPointerEvent::new(PointerPhase::Up, pos.0, pos.1) };
        deliver(shared, &up);
    }
}

/// Release the lock (re-associate the mouse, show the cursor if nothing
/// else hides it) without telling anyone. Returns whether it was held.
fn release_lock_quietly() -> bool {
    let was = CURSOR.with(|c| std::mem::replace(&mut c.borrow_mut().locked, false));
    if was {
        unsafe { CGAssociateMouseAndMouseCursorPosition(1) };
        apply_cursor();
    }
    was
}

/// A lock the platform (or the user) dropped: release and report.
fn drop_lock(shared: &Shared) {
    if release_lock_quietly() {
        if let Some(sink) = shared.sink() {
            sink.lock_changed(false);
        }
    }
}

/// The monitor body. Returns whether to swallow the event.
unsafe fn handle_event(shared: &Rc<Shared>, event: *mut NSObject) -> bool {
    let event_type: usize = msg_send![event, type];
    if event_type == policy::T_KEY_DOWN {
        let key_code: u16 = msg_send![event, keyCode];
        if key_code == policy::KEY_CODE_ESCAPE {
            drop_lock(shared);
        }
        return false;
    }
    let Some(raw) = read_raw(shared, event, event_type) else {
        return false;
    };
    let Some(ev) = policy::translate(&raw) else {
        return false;
    };
    let outcome = deliver(shared, &ev);
    policy::should_swallow(ev.phase, outcome)
}

/// Install (or, with `None`, remove) the app-level pointer source. A `Some`
/// while installed swaps the sink in place.
pub(crate) fn set_pointer_sink(backend: &mut MacosBackend, sink: Option<PointerSink>) {
    let Some(sink) = sink else {
        if let Some(p) = backend.app_pointer.take() {
            p.teardown();
        }
        return;
    };
    if let Some(p) = backend.app_pointer.as_ref() {
        *p.shared.sink.borrow_mut() = Some(sink);
        return;
    }

    let host_root = backend.host_root.clone();
    let shared = Rc::new(Shared {
        sink: RefCell::new(Some(sink)),
        host_root,
        held: Cell::new(0),
        poll_armed: Cell::new(false),
        alive: Cell::new(true),
    });

    // Crash-loud per project policy (an FFI callback that unwinds aborts).
    let block_shared = shared.clone();
    let block = RcBlock::new(move |event: *mut NSObject| -> *mut NSObject {
        let swallow =
            backend_apple_core::crash::abort_on_panic("app pointer monitor", || unsafe { handle_event(&block_shared, event) });
        if swallow {
            std::ptr::null_mut()
        } else {
            event
        }
    });
    // `addLocalMonitor…` copies the block; we keep the returned token for
    // `removeMonitor:`.
    let monitor: *mut NSObject = unsafe {
        msg_send![
            class!(NSEvent),
            addLocalMonitorForEventsMatchingMask: policy::MONITOR_MASK,
            handler: &*block,
        ]
    };
    let Some(monitor) = (unsafe { Retained::retain(monitor) }) else {
        return;
    };

    // Mouse-moved events only exist for a window that accepts them.
    let restore_moved = unsafe { host_window(&shared.host_root) }.map(|win| {
        let prev: bool = unsafe { msg_send![&*win, acceptsMouseMovedEvents] };
        let _: () = unsafe { msg_send![&*win, setAcceptsMouseMovedEvents: true] };
        (win, prev)
    });

    // Focus loss — the same two hooks the keyboard source uses: releases for
    // held buttons will not arrive, and a held lock is dropped (the user
    // switched away; the cursor must be theirs again).
    let observers = ["NSApplicationDidResignActiveNotification", "NSWindowDidResignKeyNotification"]
        .into_iter()
        .map(|name| {
            let shared = shared.clone();
            super::callbacks::observe_notification(
                backend.mtm,
                name,
                std::ptr::null_mut(),
                Rc::new(move || {
                    if !shared.alive.get() {
                        return;
                    }
                    shared.held.set(0);
                    if let Some(sink) = shared.sink() {
                        sink.focus_lost();
                    }
                    drop_lock(&shared);
                }),
            )
        })
        .collect();

    backend.app_pointer = Some(AppPointer { shared, monitor, observers, restore_moved });
}

/// Grant or release pointer lock (see the module docs). Reports through the
/// installed sink synchronously. A request without an installed source, or
/// while the app is not active, is refused.
pub(crate) fn set_pointer_lock(backend: &mut MacosBackend, locked: bool) {
    let Some(shared) = backend.app_pointer.as_ref().map(|p| p.shared.clone()) else {
        // No source: nothing could ever report a release, so never lock.
        release_lock_quietly();
        return;
    };
    if !locked {
        drop_lock(&shared);
        return;
    }
    if is_locked() {
        if let Some(sink) = shared.sink() {
            sink.lock_changed(true);
        }
        return;
    }
    let active: bool = SKIP_ACTIVE_GATE.with(Cell::get) || unsafe {
        let app: *mut AnyObject = msg_send![class!(NSApplication), sharedApplication];
        msg_send![app, isActive]
    };
    if !active {
        if let Some(sink) = shared.sink() {
            sink.lock_changed(false);
        }
        return;
    }
    unsafe {
        warp_into_window_if_outside(&shared);
        CGAssociateMouseAndMouseCursorPosition(0);
    }
    CURSOR.with(|c| c.borrow_mut().locked = true);
    apply_cursor();
    if let Some(sink) = shared.sink() {
        sink.lock_changed(true);
    }
}

/// Clicks while locked land wherever the frozen cursor sits, so a lock
/// taken with the cursor outside the window (a keyboard-triggered request)
/// first moves it to the window's content center. Inside, it stays put —
/// web parity: the cursor reappears where it was locked.
unsafe fn warp_into_window_if_outside(shared: &Shared) {
    let Some(win) = host_window(&shared.host_root) else {
        return;
    };
    let frame: CGRect = msg_send![&*win, frame];
    let content: CGRect = msg_send![&*win, contentRectForFrameRect: frame];
    let mouse: CGPoint = msg_send![class!(NSEvent), mouseLocation];
    let inside = mouse.x >= content.origin.x
        && mouse.x <= content.origin.x + content.size.width
        && mouse.y >= content.origin.y
        && mouse.y <= content.origin.y + content.size.height;
    if inside {
        return;
    }
    // `screens.firstObject` is the primary (menu-bar) screen, whose height
    // anchors the Cocoa ↔ Quartz flip. (`firstObject`, not
    // `objectAtIndex:` — `+screens` can be a Swift array bridge whose
    // signature objc2's debug message check rejects.)
    let screens: *mut AnyObject = msg_send![class!(NSScreen), screens];
    let primary: *mut AnyObject = msg_send![screens, firstObject];
    if primary.is_null() {
        return;
    }
    let primary_frame: CGRect = msg_send![primary, frame];
    let (qx, qy) = policy::cocoa_to_quartz(
        content.origin.x + content.size.width / 2.0,
        content.origin.y + content.size.height / 2.0,
        primary_frame.size.height,
    );
    CGWarpMouseCursorPosition(CGPoint { x: qx, y: qy });
    // A warp suppresses local mouse events for ~0.25 s; re-associating
    // once cancels that suppression (documented CGRemoteOperation
    // behavior), so motion reaches the sink right after the lock.
    CGAssociateMouseAndMouseCursorPosition(1);
}

/// Hide (`true`) or show the cursor. Process-global in AppKit: hidden while
/// the app is active, wherever the cursor is.
pub(crate) fn set_cursor_hidden(_backend: &mut MacosBackend, hidden: bool) {
    CURSOR.with(|c| c.borrow_mut().user_hidden = hidden);
    apply_cursor();
}

/// Whether this module currently holds a `+[NSCursor hide]` (tests).
pub(crate) fn cursor_hidden_applied() -> bool {
    CURSOR.with(|c| c.borrow().applied_hidden)
}
