//! App-level pointer source, pointer lock and cursor hiding for the iOS
//! backend (iPadOS with a mouse or trackpad). Drives
//! `IosBackend::{set_pointer_sink_impl, set_pointer_lock_impl,
//! set_cursor_hidden_impl}`; the pure halves live in
//! `crate::pointer_input_policy`.
//!
//! - **Buttons + motion** come from GameController's `GCMouse`
//!   (`GCMouse.mice`, plus `GCMouseDidConnectNotification` for mice that
//!   arrive later): `mouseInput.mouseMovedHandler` gives movement deltas,
//!   and the left / right / middle / auxiliary buttons'
//!   `pressedChangedHandler`s give app-wide downs and ups. This works
//!   whether or not the pointer is locked and whatever view is under it,
//!   and never sees finger touches.
//! - **Position**: GameController has none. A `UIHoverGestureRecognizer` on
//!   the host root tracks it (top-left origin, points = logical px) while
//!   the pointer hovers; during a press (UIKit turns the pointer into a
//!   touch, so hover pauses) and while locked, the last known position is
//!   reported — the lock contract says the position stays put anyway.
//! - **Wheel**: unlocked, a scroll-only `UIPanGestureRecognizer`
//!   (`allowedScrollTypesMask = all`, no touch types) on the host root,
//!   recognizing simultaneously with everything so scroll views keep
//!   scrolling; deltas in points. Locked, UIKit delivers no pointer scroll,
//!   so `GCMouseInput.scroll` is used instead (see the policy module docs).
//! - **PreventDefault is not honored on iOS**: GameController and the
//!   simultaneous recognizers observe input, they cannot cancel UIKit's
//!   own handling of the same press / scroll. Documented, not faked.
//! - **Lock**: iPadOS 14+ locks the pointer for a view controller whose
//!   `prefersPointerLocked` is true, and only for a full-screen,
//!   foreground scene with a pointer connected. The backend does not own
//!   the root view controller (the generated Swift host does), so it asks
//!   through the same soft-fail protocol `fullscreen_setter` uses: the
//!   root VC's `applyPointerLock:` (which flips `prefersPointerLocked` and
//!   calls `setNeedsUpdateOfPrefersPointerLocked`). A host without it, or
//!   a scene without `pointerLockState` (iPhone), refuses: `false`. The
//!   result arrives through `UIPointerLockStateDidChangeNotification`; a
//!   lock the system drops (app switcher, Split View, pointer gone) is
//!   reported `false` and the preference is cleared so UIKit does not
//!   re-lock behind the user's back.
//! - **Cursor hiding**: a `UIPointerInteraction` on the host root whose
//!   delegate returns `+[UIPointerStyle hiddenPointerStyle]`. Controls with
//!   their own pointer interaction (buttons with a hover effect) still
//!   style the pointer over themselves.
//! - **Focus loss**: `UIApplicationWillResignActiveNotification` (the hook
//!   the keyboard source uses) and a mouse disconnecting call
//!   `PointerSink::focus_lost` on the next main-queue turn.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGPoint, MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::UIView;
use runtime_shared::pointer_input::{AppPointerEvent, PointerPhase, PointerSink};

use super::IosBackend;
use crate::pointer_input_policy::{self as policy, GcButton, LockRequest, LockState};

// GameController is linked through `[package.metadata.idealyst.ios]
// frameworks` too (Cargo.toml): the app's final link is done by
// Xcode/swiftc on a staticlib, which drops this attribute.
#[link(name = "GameController", kind = "framework")]
extern "C" {
    static GCMouseDidConnectNotification: &'static NSString;
    static GCMouseDidDisconnectNotification: &'static NSString;
}

extern "C" {
    static UIPointerLockStateDidChangeNotification: &'static NSString;
}

/// `UIGestureRecognizerState` values the scroll pan reads.
const GR_BEGAN: isize = 1;
const GR_CHANGED: isize = 2;
const GR_ENDED: isize = 3;
/// `UIScrollTypeMask.all` (discrete wheel | continuous trackpad).
const SCROLL_TYPE_MASK_ALL: isize = 3;

thread_local! {
    /// `set_cursor_hidden` state, read by the pointer-style delegate.
    static CURSOR_HIDDEN: Cell<bool> = const { Cell::new(false) };
}

/// State shared with the target's methods and the GameController blocks.
struct Shared {
    /// Callers CLONE the sink out and drop the borrow before calling it: a
    /// listener may add/remove listeners, which re-enters
    /// `set_pointer_sink` on the backend.
    sink: RefCell<Option<PointerSink>>,
    host_root: Retained<UIView>,
    position: Cell<(f32, f32)>,
    pan_last: Cell<(f64, f64)>,
    lock: RefCell<LockState>,
    /// Mice whose handlers we set (cleared on teardown).
    mice: RefCell<Vec<Retained<AnyObject>>>,
    /// Cleared on teardown so a queued microtask / late handler does nothing.
    alive: Cell<bool>,
}

impl Shared {
    fn sink(&self) -> Option<PointerSink> {
        if !self.alive.get() {
            return None;
        }
        self.sink.borrow().clone()
    }

    fn locked(&self) -> bool {
        self.lock.borrow().locked
    }

    /// Deliver one event. The outcome is ignored — see the module docs.
    fn deliver(&self, mut ev: AppPointerEvent) {
        let (shift, ctrl, alt, meta) = read_modifiers();
        ev.shift = shift;
        ev.ctrl = ctrl;
        ev.alt = alt;
        ev.meta = meta;
        if let Some(sink) = self.sink() {
            let _ = sink.event(&ev);
        }
    }

    fn event_at(&self, phase: PointerPhase) -> AppPointerEvent {
        let (x, y) = self.position.get();
        AppPointerEvent::new(phase, x, y)
    }
}

/// Modifier keys from the coalesced hardware keyboard (GameController
/// pointer input carries none of its own).
fn read_modifiers() -> (bool, bool, bool, bool) {
    let input: *mut AnyObject = unsafe {
        let kb: *mut AnyObject = msg_send![objc2::class!(GCKeyboard), coalescedKeyboard];
        if kb.is_null() {
            return (false, false, false, false);
        }
        msg_send![kb, keyboardInput]
    };
    if input.is_null() {
        return (false, false, false, false);
    }
    policy::modifiers(|usage| unsafe {
        let button: *mut AnyObject = msg_send![input, buttonForKeyCode: usage];
        !button.is_null() && {
            let pressed: bool = msg_send![button, isPressed];
            pressed
        }
    })
}

pub(crate) struct PointerTargetIvars {
    shared: Rc<Shared>,
}

declare_class!(
    /// Target of the hover / scroll recognizers and observer of the
    /// focus-loss, mouse-connect and pointer-lock notifications.
    pub(crate) struct IdealystPointerTarget;

    unsafe impl ClassType for IdealystPointerTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystPointerTarget";
    }

    impl DeclaredClass for IdealystPointerTarget {
        type Ivars = PointerTargetIvars;
    }

    unsafe impl NSObjectProtocol for IdealystPointerTarget {}

    unsafe impl IdealystPointerTarget {
        #[method(hover:)]
        fn hover(&self, recognizer: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystPointerTarget::hover", || {
                let s = &self.ivars().shared;
                let p: CGPoint = unsafe { msg_send![recognizer, locationInView: &*s.host_root] };
                s.position.set((p.x as f32, p.y as f32));
            });
        }

        #[method(scrollPan:)]
        fn scroll_pan(&self, recognizer: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystPointerTarget::scrollPan", || {
                let s = &self.ivars().shared;
                let state: isize = unsafe { msg_send![recognizer, state] };
                if state == GR_BEGAN {
                    s.pan_last.set((0.0, 0.0));
                }
                if !matches!(state, GR_BEGAN | GR_CHANGED | GR_ENDED) {
                    return;
                }
                let p: CGPoint = unsafe { msg_send![recognizer, locationInView: &*s.host_root] };
                s.position.set((p.x as f32, p.y as f32));
                let t: CGPoint = unsafe { msg_send![recognizer, translationInView: &*s.host_root] };
                let (wx, wy) = policy::pan_wheel_step(s.pan_last.get(), (t.x, t.y));
                s.pan_last.set((t.x, t.y));
                // Locked: the GCMouse scroll handler owns the wheel.
                if s.locked() || (wx == 0.0 && wy == 0.0) {
                    return;
                }
                let mut ev = s.event_at(PointerPhase::Wheel);
                ev.wheel_dx = wx;
                ev.wheel_dy = wy;
                s.deliver(ev);
            });
        }

        #[method(gestureRecognizer:shouldRecognizeSimultaneouslyWithGestureRecognizer:)]
        fn simultaneous(&self, _a: *mut AnyObject, _b: *mut AnyObject) -> Bool {
            // Observe only: never block a scroll view's own pan.
            Bool::YES
        }

        #[method(mouseDidConnect:)]
        fn mouse_did_connect(&self, note: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystPointerTarget::mouseDidConnect", || {
                let mouse: *mut AnyObject = unsafe { msg_send![note, object] };
                if let Some(mouse) = unsafe { Retained::retain(mouse) } {
                    attach_mouse(&self.ivars().shared, mouse);
                }
            });
        }

        #[method(mouseDidDisconnect:)]
        fn mouse_did_disconnect(&self, _note: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystPointerTarget::mouseDidDisconnect", || {
                // Releases for buttons held on the vanished mouse never come.
                defer_focus_lost(&self.ivars().shared);
            });
        }

        #[method(appWillResignActive:)]
        fn app_will_resign_active(&self, _note: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystPointerTarget::appWillResignActive", || {
                defer_focus_lost(&self.ivars().shared);
            });
        }

        #[method(pointerLockDidChange:)]
        fn pointer_lock_did_change(&self, _note: *mut AnyObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystPointerTarget::pointerLockDidChange", || {
                sync_lock(&self.ivars().shared);
            });
        }
    }
);

impl IdealystPointerTarget {
    fn new(mtm: MainThreadMarker, shared: Rc<Shared>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(PointerTargetIvars { shared });
        unsafe { msg_send_id![super(this), init] }
    }
}

/// `focus_lost` on the NEXT main-queue turn (UIKit may post inside work the
/// framework started; the synthesized releases run author listeners).
fn defer_focus_lost(shared: &Rc<Shared>) {
    let weak = Rc::downgrade(shared);
    runtime_shared::schedule_microtask(move || {
        if let Some(sink) = weak.upgrade().and_then(|s| s.sink()) {
            sink.focus_lost();
        }
    });
}

type MovedBlock = dyn Fn(*mut AnyObject, f32, f32);
type PressedBlock = dyn Fn(*mut AnyObject, f32, Bool);
type ValueBlock = dyn Fn(*mut AnyObject, f32, f32);

/// Set our handlers on one `GCMouse`. The blocks hold the state weakly and
/// do nothing after teardown.
fn attach_mouse(shared: &Rc<Shared>, mouse: Retained<AnyObject>) {
    if shared.mice.borrow().iter().any(|m| Retained::as_ptr(m) == Retained::as_ptr(&mouse)) {
        return;
    }
    let input: *mut AnyObject = unsafe { msg_send![&*mouse, mouseInput] };
    if input.is_null() {
        return;
    }

    let weak = Rc::downgrade(shared);
    let moved: RcBlock<MovedBlock> = RcBlock::new(move |_input: *mut AnyObject, dx: f32, dy: f32| {
        crate::imp::ffi_guard::guard_ffi("GCMouse mouseMovedHandler", || {
            let Some(s) = weak.upgrade() else { return };
            let (dx, dy) = policy::gc_motion(dx, dy);
            let mut ev = s.event_at(PointerPhase::Move);
            ev.dx = dx;
            ev.dy = dy;
            s.deliver(ev);
        });
    });
    unsafe {
        let _: () = msg_send![input, setMouseMovedHandler: &*moved];
    }

    let mut buttons: Vec<(GcButton, *mut AnyObject)> = Vec::new();
    unsafe {
        buttons.push((GcButton::Left, msg_send![input, leftButton]));
        buttons.push((GcButton::Right, msg_send![input, rightButton]));
        buttons.push((GcButton::Middle, msg_send![input, middleButton]));
        let aux: *mut AnyObject = msg_send![input, auxiliaryButtons];
        if !aux.is_null() {
            let n: usize = msg_send![aux, count];
            for i in 0..n {
                buttons.push((GcButton::Aux(i), msg_send![aux, objectAtIndex: i]));
            }
        }
    }
    for (which, button) in buttons {
        if button.is_null() {
            continue;
        }
        let Some(mb) = policy::button_for(which) else { continue };
        let weak = Rc::downgrade(shared);
        let block: RcBlock<PressedBlock> = RcBlock::new(move |_b: *mut AnyObject, _value: f32, pressed: Bool| {
            crate::imp::ffi_guard::guard_ffi("GCMouse pressedChangedHandler", || {
                let Some(s) = weak.upgrade() else { return };
                let phase = if pressed.as_bool() { PointerPhase::Down } else { PointerPhase::Up };
                let mut ev = s.event_at(phase);
                ev.button = Some(mb);
                s.deliver(ev);
            });
        });
        unsafe {
            let _: () = msg_send![button, setPressedChangedHandler: &*block];
        }
    }

    let scroll: *mut AnyObject = unsafe { msg_send![input, scroll] };
    if !scroll.is_null() {
        let weak = Rc::downgrade(shared);
        let block: RcBlock<ValueBlock> = RcBlock::new(move |_pad: *mut AnyObject, x: f32, y: f32| {
            crate::imp::ffi_guard::guard_ffi("GCMouse scroll valueChangedHandler", || {
                let Some(s) = weak.upgrade() else { return };
                // Unlocked, the UIKit scroll pan owns the wheel (points,
                // natural-scrolling aware); see the module docs.
                if !s.locked() || (x == 0.0 && y == 0.0) {
                    return;
                }
                let (wx, wy) = policy::gc_scroll_px(x, y);
                let mut ev = s.event_at(PointerPhase::Wheel);
                ev.wheel_dx = wx;
                ev.wheel_dy = wy;
                s.deliver(ev);
            });
        });
        unsafe {
            let _: () = msg_send![scroll, setValueChangedHandler: &*block];
        }
    }
    shared.mice.borrow_mut().push(mouse);
}

/// Clear every handler [`attach_mouse`] set on `mouse`.
fn detach_mouse(mouse: &AnyObject) {
    unsafe {
        let input: *mut AnyObject = msg_send![mouse, mouseInput];
        if input.is_null() {
            return;
        }
        let none_moved: *const block2::Block<MovedBlock> = std::ptr::null();
        let _: () = msg_send![input, setMouseMovedHandler: none_moved];
        let none_pressed: *const block2::Block<PressedBlock> = std::ptr::null();
        for sel_button in [
            msg_send![input, leftButton],
            msg_send![input, rightButton],
            msg_send![input, middleButton],
        ] {
            let b: *mut AnyObject = sel_button;
            if !b.is_null() {
                let _: () = msg_send![b, setPressedChangedHandler: none_pressed];
            }
        }
        let aux: *mut AnyObject = msg_send![input, auxiliaryButtons];
        if !aux.is_null() {
            let n: usize = msg_send![aux, count];
            for i in 0..n {
                let b: *mut AnyObject = msg_send![aux, objectAtIndex: i];
                let _: () = msg_send![b, setPressedChangedHandler: none_pressed];
            }
        }
        let scroll: *mut AnyObject = msg_send![input, scroll];
        if !scroll.is_null() {
            let none_value: *const block2::Block<ValueBlock> = std::ptr::null();
            let _: () = msg_send![scroll, setValueChangedHandler: none_value];
        }
    }
}

/// The host root's window, root view controller (only if it takes
/// `applyPointerLock:`), and the scene's `UIPointerLockState` (nil on
/// iPhone).
fn lock_hooks(shared: &Shared) -> (Option<*mut AnyObject>, Option<*mut AnyObject>) {
    unsafe {
        let window: *mut AnyObject = msg_send![&*shared.host_root, window];
        if window.is_null() {
            return (None, None);
        }
        let vc: *mut AnyObject = msg_send![window, rootViewController];
        let vc = (!vc.is_null()
            && msg_send![vc, respondsToSelector: objc2::sel!(applyPointerLock:)])
        .then_some(vc);
        let scene: *mut AnyObject = msg_send![window, windowScene];
        let state: *mut AnyObject = if scene.is_null() { std::ptr::null_mut() } else { msg_send![scene, pointerLockState] };
        (vc, (!state.is_null()).then_some(state))
    }
}

fn platform_locked(state: Option<*mut AnyObject>) -> bool {
    state.is_some_and(|st| unsafe { msg_send![st, isLocked] })
}

fn apply_preference(vc: Option<*mut AnyObject>, locked: bool) {
    if let Some(vc) = vc {
        let _: () = unsafe { msg_send![vc, applyPointerLock: locked] };
    }
}

/// Re-read the scene's lock state and report a change.
fn sync_lock(shared: &Rc<Shared>) {
    if !shared.alive.get() {
        return;
    }
    let (vc, state) = lock_hooks(shared);
    let change = shared.lock.borrow_mut().platform_changed(platform_locked(state));
    if change.clear_preference {
        apply_preference(vc, false);
    }
    if let (Some(locked), Some(sink)) = (change.report, shared.sink()) {
        sink.lock_changed(locked);
    }
}

/// The installed app-level pointer source.
pub(crate) struct AppPointer {
    shared: Rc<Shared>,
    target: Retained<IdealystPointerTarget>,
    recognizers: Vec<Retained<AnyObject>>,
}

impl AppPointer {
    fn teardown(self) {
        let shared = &self.shared;
        // Release a lock we asked for: nothing would report it any more.
        if shared.lock.borrow_mut().release() || shared.locked() {
            let (vc, _) = lock_hooks(shared);
            apply_preference(vc, false);
        }
        shared.alive.set(false);
        shared.sink.borrow_mut().take();
        unsafe {
            let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
            let _: () = msg_send![center, removeObserver: &*self.target];
            for r in &self.recognizers {
                let _: () = msg_send![&*shared.host_root, removeGestureRecognizer: &**r];
            }
        }
        for mouse in shared.mice.borrow_mut().drain(..) {
            detach_mouse(&mouse);
        }
    }
}

fn observe(target: &IdealystPointerTarget, selector: objc2::runtime::Sel, name: &NSString) {
    unsafe {
        let center: *mut AnyObject = msg_send![objc2::class!(NSNotificationCenter), defaultCenter];
        let nil: *mut AnyObject = std::ptr::null_mut();
        let _: () = msg_send![center, addObserver: target, selector: selector, name: name, object: nil];
    }
}

/// Install (or, with `None`, remove) the app-level pointer source. A `Some`
/// while installed swaps the sink in place.
pub(crate) fn set_pointer_sink(backend: &mut IosBackend, sink: Option<PointerSink>) {
    let Some(sink) = sink else {
        if let Some(p) = backend.app_pointer.take() {
            p.teardown();
        }
        // The dispatcher only drops the source once nothing (cursor hiding
        // included) needs it; never leave the pointer hidden behind it.
        hide_cursor(backend, false);
        return;
    };
    if let Some(p) = backend.app_pointer.as_ref() {
        *p.shared.sink.borrow_mut() = Some(sink);
        return;
    }
    let Some(host_root) = backend.host_root.clone() else {
        return;
    };
    let shared = Rc::new(Shared {
        sink: RefCell::new(Some(sink)),
        host_root: host_root.clone(),
        position: Cell::new((0.0, 0.0)),
        pan_last: Cell::new((0.0, 0.0)),
        lock: RefCell::new(LockState::default()),
        mice: RefCell::new(Vec::new()),
        alive: Cell::new(true),
    });
    let target = IdealystPointerTarget::new(backend.mtm, shared.clone());

    let mut recognizers = Vec::new();
    unsafe {
        let hover: Retained<AnyObject> = msg_send_id![
            msg_send_id![objc2::class!(UIHoverGestureRecognizer), alloc],
            initWithTarget: &*target,
            action: objc2::sel!(hover:),
        ];
        let _: () = msg_send![&*host_root, addGestureRecognizer: &*hover];
        recognizers.push(hover);

        let pan: Retained<AnyObject> = msg_send_id![
            msg_send_id![objc2::class!(UIPanGestureRecognizer), alloc],
            initWithTarget: &*target,
            action: objc2::sel!(scrollPan:),
        ];
        // Scroll events only: no finger / pencil touches.
        let no_touch_types: Retained<AnyObject> = msg_send_id![objc2::class!(NSArray), array];
        let _: () = msg_send![&*pan, setAllowedTouchTypes: &*no_touch_types];
        let _: () = msg_send![&*pan, setAllowedScrollTypesMask: SCROLL_TYPE_MASK_ALL];
        let _: () = msg_send![&*pan, setCancelsTouchesInView: false];
        let _: () = msg_send![&*pan, setDelegate: &*target];
        let _: () = msg_send![&*host_root, addGestureRecognizer: &*pan];
        recognizers.push(pan);

        observe(&target, objc2::sel!(mouseDidConnect:), GCMouseDidConnectNotification);
        observe(&target, objc2::sel!(mouseDidDisconnect:), GCMouseDidDisconnectNotification);
        observe(&target, objc2::sel!(pointerLockDidChange:), UIPointerLockStateDidChangeNotification);
        observe(
            &target,
            objc2::sel!(appWillResignActive:),
            &NSString::from_str("UIApplicationWillResignActiveNotification"),
        );

        let mice: *mut AnyObject = msg_send![objc2::class!(GCMouse), mice];
        if !mice.is_null() {
            let n: usize = msg_send![mice, count];
            for i in 0..n {
                let m: *mut AnyObject = msg_send![mice, objectAtIndex: i];
                if let Some(m) = Retained::retain(m) {
                    attach_mouse(&shared, m);
                }
            }
        }
    }
    backend.app_pointer = Some(AppPointer { shared, target, recognizers });
}

/// Ask for or release pointer lock (see the module docs). A refusal is
/// reported synchronously; a grant arrives with the platform notification.
pub(crate) fn set_pointer_lock(backend: &mut IosBackend, locked: bool) {
    let Some(shared) = backend.app_pointer.as_ref().map(|p| p.shared.clone()) else {
        return;
    };
    let (vc, state) = lock_hooks(&shared);
    if locked {
        let request = shared.lock.borrow_mut().request(vc.is_some() && state.is_some(), platform_locked(state));
        match request {
            LockRequest::Report(v) => {
                if let Some(sink) = shared.sink() {
                    sink.lock_changed(v);
                }
            }
            LockRequest::AskPlatform => apply_preference(vc, true),
        }
    } else if shared.lock.borrow_mut().release() {
        apply_preference(vc, false);
    }
}

pub(crate) struct CursorHiderIvars {}

declare_class!(
    /// `UIPointerInteractionDelegate` that hides the pointer while
    /// `set_cursor_hidden(true)` is in effect.
    pub(crate) struct IdealystCursorHider;

    unsafe impl ClassType for IdealystCursorHider {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystCursorHider";
    }

    impl DeclaredClass for IdealystCursorHider {
        type Ivars = CursorHiderIvars;
    }

    unsafe impl NSObjectProtocol for IdealystCursorHider {}

    unsafe impl IdealystCursorHider {
        #[method(pointerInteraction:styleForRegion:)]
        fn style_for_region(&self, _interaction: *mut AnyObject, _region: *mut AnyObject) -> *mut AnyObject {
            crate::imp::ffi_guard::guard_ffi("IdealystCursorHider::styleForRegion", || {
                if CURSOR_HIDDEN.with(Cell::get) {
                    unsafe { msg_send![objc2::class!(UIPointerStyle), hiddenPointerStyle] }
                } else {
                    // nil = the system's default pointer.
                    std::ptr::null_mut()
                }
            })
        }
    }
);

/// The installed cursor hider: the interaction on the host root and its
/// delegate (UIKit holds delegates weakly).
pub(crate) struct CursorHider {
    interaction: Retained<AnyObject>,
    _delegate: Retained<IdealystCursorHider>,
}

fn hide_cursor(backend: &mut IosBackend, hidden: bool) {
    CURSOR_HIDDEN.with(|c| c.set(hidden));
    let Some(host_root) = backend.host_root.clone() else {
        return;
    };
    if hidden {
        if backend.cursor_hider.is_none() {
            let delegate: Retained<IdealystCursorHider> = {
                let this = backend.mtm.alloc::<IdealystCursorHider>();
                let this = this.set_ivars(CursorHiderIvars {});
                unsafe { msg_send_id![super(this), init] }
            };
            let interaction: Retained<AnyObject> = unsafe {
                msg_send_id![msg_send_id![objc2::class!(UIPointerInteraction), alloc], initWithDelegate: &*delegate]
            };
            unsafe {
                let _: () = msg_send![&*host_root, addInteraction: &*interaction];
            }
            backend.cursor_hider = Some(CursorHider { interaction, _delegate: delegate });
        }
        if let Some(h) = backend.cursor_hider.as_ref() {
            // Re-query the style for the pointer's current region.
            let _: () = unsafe { msg_send![&*h.interaction, invalidate] };
        }
    } else if let Some(h) = backend.cursor_hider.take() {
        unsafe {
            let _: () = msg_send![&*host_root, removeInteraction: &*h.interaction];
        }
    }
}

/// Hide (`true`) or show the pointer over the app.
pub(crate) fn set_cursor_hidden(backend: &mut IosBackend, hidden: bool) {
    hide_cursor(backend, hidden);
}
