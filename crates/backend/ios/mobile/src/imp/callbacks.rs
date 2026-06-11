use runtime_core::primitives::key::{KeyDownHandler, KeyEvent, KeyOutcome};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, MainThreadMarker, NSObject, NSRange, NSString};
use objc2_ui_kit::{UIScrollView, UITextField, UITextView, UIView, UIWindow};
use std::cell::RefCell;
use std::rc::Rc;

/// `UIEdgeInsets` — UIKit's per-side rect inset struct. objc2-foundation
/// doesn't ship this type; we declare it here with a matching `Encode`
/// so `msg_send![view, safeAreaInsets]` returns the correct layout.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub(crate) struct UIEdgeInsets {
    pub top: CGFloat,
    pub left: CGFloat,
    pub bottom: CGFloat,
    pub right: CGFloat,
}

unsafe impl Encode for UIEdgeInsets {
    const ENCODING: Encoding = Encoding::Struct(
        "UIEdgeInsets",
        &[CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING],
    );
}

// =========================================================================
// CallbackTarget — ObjC action target that calls a Rust closure
// =========================================================================

extern "C" {
    fn CACurrentMediaTime() -> f64;
}

/// Seconds a tap-driven view must have been on-screen before its tap
/// recognizer is allowed to fire. Prevents the spurious "Pressable
/// fires `on_click` at mount" bug: UIKit intermittently delivers a
/// phantom touch sequence to a freshly-mounted view during the same
/// run-loop turn it enters the window (most visibly the appointment
/// block under the viewport center on QuillEMR's Schedule, which
/// auto-opened the detail modal with no user tap). A real user tap
/// cannot physically occur before the screen has rendered + the
/// human has reacted, so gating the first ~third of a second after
/// the view acquires a window discards only synthetic taps. Applies
/// to both Link and Pressable (they share this target/delegate).
const TAP_GATE_SETTLE_SECS: f64 = 0.35;

/// Pure decision for the tap gate, factored out so the timing logic is
/// unit-testable without a live UIKit window. Given the previously
/// stamped window-entry time (`< 0.0` == not yet stamped) and the
/// current media time, returns `(allow, new_entry)`:
///   - first call (`entry < 0.0`): stamp `now`, reject (the synthetic
///     mount-time tap arrives in this same run-loop turn);
///   - later call: allow only once `now - entry >= TAP_GATE_SETTLE_SECS`,
///     leaving the stamp unchanged.
fn tap_gate_decision(entry: f64, now: f64) -> (bool, f64) {
    if entry < 0.0 {
        return (false, now);
    }
    (now - entry >= TAP_GATE_SETTLE_SECS, entry)
}

pub struct CallbackTargetIvars {
    callback: RefCell<Option<Rc<dyn Fn()>>>,
    /// `CACurrentMediaTime()` stamped the first time this target's
    /// recognizer is asked `gestureRecognizerShouldBegin:` with its
    /// view in a window. `< 0.0` = not yet stamped. See
    /// `TAP_GATE_SETTLE_SECS` for the bug this guards.
    window_entry: std::cell::Cell<f64>,
}

declare_class!(
    pub struct CallbackTarget;

    unsafe impl ClassType for CallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystCallbackTarget";
    }

    impl DeclaredClass for CallbackTarget {
        type Ivars = CallbackTargetIvars;
    }

    unsafe impl CallbackTarget {
        #[method(invoke)]
        fn invoke(&self) {
            // ObjC action target (extern "C" IMP). Guard author callback
            // code: a panic must abort, not unwind into UIKit dispatch.
            crate::imp::ffi_guard::guard_ffi("CallbackTarget::invoke", || {
                let ivars = self.ivars();
                if let Some(cb) = ivars.callback.borrow().as_ref() {
                    cb();
                }
            })
        }

        /// `UIGestureRecognizerDelegate.gestureRecognizerShouldBegin:`.
        /// Rejects taps that begin before the recognizer's view has
        /// been in a window for `TAP_GATE_SETTLE_SECS` — the
        /// mount-time phantom-tap guard documented on that constant.
        /// Recognizers we don't wire as Link/Pressable tap gates never
        /// reach here (we only set this object as their delegate).
        #[method(gestureRecognizerShouldBegin:)]
        fn gesture_recognizer_should_begin(
            &self,
            recognizer: &objc2_ui_kit::UIGestureRecognizer,
        ) -> objc2::runtime::Bool {
            let view: *mut UIView = unsafe { msg_send![recognizer, view] };
            if view.is_null() {
                return objc2::runtime::Bool::NO;
            }
            // `-[UIView window]` is nil until the view is in a window.
            // Typed as a bare object pointer to avoid pulling in the
            // `UIWindow` objc2 feature for a null check.
            let window: *mut objc2::runtime::AnyObject =
                unsafe { msg_send![view, window] };
            if window.is_null() {
                // Not on screen yet — no legitimate tap is possible.
                return objc2::runtime::Bool::NO;
            }
            let now = unsafe { CACurrentMediaTime() };
            let (allow, new_entry) =
                tap_gate_decision(self.ivars().window_entry.get(), now);
            self.ivars().window_entry.set(new_entry);
            objc2::runtime::Bool::new(allow)
        }
    }
);

impl CallbackTarget {
    pub fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn()>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        // Stamp the settle clock at CREATION. The control is built immediately
        // before it's mounted (navigator screens build on push, modals on open),
        // so create-time ≈ on-screen-time. The phantom mount-tap fires within the
        // same run-loop turn (≪ TAP_GATE_SETTLE_SECS after create) → rejected; a
        // real human tap is always >0.35s after create → allowed on the FIRST try.
        //
        // The OLD behavior stamped on the first `gestureRecognizerShouldBegin:`
        // call and rejected it outright — which assumed a phantom tap always
        // arrives first. On a pushed screen with NO phantom (e.g. the whiteboard's
        // Settings), that ate the user's first real tap on every Pressable/Link
        // (SegmentedControl, Switch, …). Anchoring on create-time fixes that
        // without re-opening the mount-time auto-open bug for immediate mounts.
        let now = unsafe { CACurrentMediaTime() };
        let this = this.set_ivars(CallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
            window_entry: std::cell::Cell::new(now),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// BoolCallbackTarget — for UISwitch
// =========================================================================

pub(crate) struct BoolCallbackTargetIvars {
    callback: RefCell<Option<Rc<dyn Fn(bool)>>>,
}

declare_class!(
    pub(crate) struct BoolCallbackTarget;

    unsafe impl ClassType for BoolCallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystBoolCallbackTarget";
    }

    impl DeclaredClass for BoolCallbackTarget {
        type Ivars = BoolCallbackTargetIvars;
    }

    unsafe impl BoolCallbackTarget {
        #[method(invoke:)]
        fn invoke(&self, sender: &NSObject) {
            crate::imp::ffi_guard::guard_ffi("BoolCallbackTarget::invoke", || {
                let ivars = self.ivars();
                if let Some(cb) = ivars.callback.borrow().as_ref() {
                    let is_on: bool = unsafe { msg_send![sender, isOn] };
                    cb(is_on);
                }
            })
        }
    }
);

impl BoolCallbackTarget {
    pub(crate) fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn(bool)>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(BoolCallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// FloatCallbackTarget — for UISlider
// =========================================================================

pub(crate) struct FloatCallbackTargetIvars {
    callback: RefCell<Option<Rc<dyn Fn(f32)>>>,
}

declare_class!(
    pub(crate) struct FloatCallbackTarget;

    unsafe impl ClassType for FloatCallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystFloatCallbackTarget";
    }

    impl DeclaredClass for FloatCallbackTarget {
        type Ivars = FloatCallbackTargetIvars;
    }

    unsafe impl FloatCallbackTarget {
        #[method(invoke:)]
        fn invoke(&self, sender: &NSObject) {
            crate::imp::ffi_guard::guard_ffi("FloatCallbackTarget::invoke", || {
                let ivars = self.ivars();
                if let Some(cb) = ivars.callback.borrow().as_ref() {
                    let value: f32 = unsafe { msg_send![sender, value] };
                    cb(value);
                }
            })
        }
    }
);

impl FloatCallbackTarget {
    pub(crate) fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn(f32)>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(FloatCallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// StringCallbackTarget — for UITextField
// =========================================================================

pub(crate) struct StringCallbackTargetIvars {
    callback: RefCell<Option<Rc<dyn Fn(String)>>>,
}

declare_class!(
    pub(crate) struct StringCallbackTarget;

    unsafe impl ClassType for StringCallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystStringCallbackTarget";
    }

    impl DeclaredClass for StringCallbackTarget {
        type Ivars = StringCallbackTargetIvars;
    }

    unsafe impl StringCallbackTarget {
        #[method(invoke:)]
        fn invoke(&self, sender: &NSObject) {
            crate::imp::ffi_guard::guard_ffi("StringCallbackTarget::invoke", || {
                let ivars = self.ivars();
                if let Some(cb) = ivars.callback.borrow().as_ref() {
                    let text: Option<Retained<NSString>> = unsafe { msg_send_id![sender, text] };
                    let s = text.map(|ns| ns.to_string()).unwrap_or_default();
                    cb(s);
                }
            })
        }
    }
);

impl StringCallbackTarget {
    pub(crate) fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn(String)>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(StringCallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// MetalView — UIView subclass backed by CAMetalLayer
// =========================================================================
//
// Lifecycle methods drive the framework's `Graphics` primitive
// callbacks:
//
//   layoutSubviews
//     ├─ first call with non-zero bounds → on_ready (once per surface)
//     └─ subsequent calls with new size  → on_resize
//
//   willMoveToSuperview(nil) → on_lost
//
// State lives on the subclass's ivars rather than in capturing
// closures, so the three callbacks stay reachable past
// `create_graphics`'s return without leaking the slot the way the
// previous `performSelector:withDelay:0` shape required (see the
// `mem::forget` keepalive in
// `examples/website/src/components/simulator.rs`).

use std::cell::Cell;
use runtime_core::primitives::graphics::{
    GraphicsSurface, OnLost, OnReady, OnReadyEvent, OnResize, OnResizeEvent,
};
use objc2_foundation::CGRect;

pub(crate) struct MetalViewIvars {
    pub(crate) on_ready: RefCell<Option<OnReady>>,
    pub(crate) on_resize: RefCell<Option<OnResize>>,
    pub(crate) on_lost: RefCell<Option<OnLost>>,
    pub(crate) surface: RefCell<Option<GraphicsSurface>>,
    /// Physical-pixel size last reported via `on_ready` / `on_resize`.
    /// `(0, 0)` is the sentinel "haven't reported yet" — the first
    /// `layoutSubviews` past zero bounds fires `on_ready` and
    /// transitions out of that state.
    pub(crate) last_size: Cell<(u32, u32)>,
    /// `true` once `on_ready` has been delivered for the current
    /// surface. Cleared by `willMoveToSuperview:nil` so a re-add
    /// could re-fire (matches the trait's "Mount → on_ready → on_lost
    /// → on_ready → … → unmount" contract).
    pub(crate) ready_fired: Cell<bool>,
}

declare_class!(
    pub(crate) struct MetalView;

    unsafe impl ClassType for MetalView {
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMetalView";
    }

    impl DeclaredClass for MetalView {
        type Ivars = MetalViewIvars;
    }

    unsafe impl MetalView {
        /// Override +layerClass to return [CAMetalLayer class]. Return
        /// type must be `&'static AnyClass` (objc encoding `#`); a raw
        /// pointer encodes as `^v` and objc2-0.5+ rejects the class
        /// declaration with a runtime panic during `register_class`.
        #[method(layerClass)]
        fn layer_class() -> &'static objc2::runtime::AnyClass {
            objc2::class!(CAMetalLayer)
        }

        /// UIKit calls this whenever the view needs to lay out its
        /// subviews (after `setBounds:` / `setFrame:`, on screen
        /// rotation, when the parent's autoresizing kicks in, …).
        /// We use it to detect the first non-zero bounds (fire
        /// `on_ready`) and any subsequent size change (`on_resize`).
        #[method(layoutSubviews)]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            let frame: CGRect = unsafe { msg_send![self, frame] };
            let scale: CGFloat = unsafe { msg_send![self, contentScaleFactor] };
            let w = (frame.size.width * scale).max(0.0) as u32;
            let h = (frame.size.height * scale).max(0.0) as u32;
            if w == 0 || h == 0 {
                // Still zero-sized — pre-layout call. Wait.
                return;
            }
            let new_size = (w, h);
            let prev_size = self.ivars().last_size.get();
            if !self.ivars().ready_fired.get() {
                // First viable bounds — fire on_ready.
                let surface = match self.ivars().surface.borrow().clone() {
                    Some(s) => s,
                    None => return, // no surface installed yet (shouldn't happen)
                };
                let mut handler = self.ivars().on_ready.borrow_mut();
                if let Some(cb) = handler.as_mut() {
                    cb(OnReadyEvent {
                        surface,
                        size: new_size,
                        // iOS rides canvas-native (no vello yet); 1.0 keeps the
                        // physical-size contract until iOS GPU canvas is wired.
                        scale: 1.0,
                    });
                }
                self.ivars().ready_fired.set(true);
                self.ivars().last_size.set(new_size);
                return;
            }
            if new_size == prev_size {
                return;
            }
            // Bounds changed after on_ready — fire on_resize.
            let mut handler = self.ivars().on_resize.borrow_mut();
            if let Some(cb) = handler.as_mut() {
                cb(OnResizeEvent { size: new_size, scale: 1.0 });
            }
            self.ivars().last_size.set(new_size);
        }

        /// Called whenever the view is about to be re-parented —
        /// `newSuperview` is `nil` when the view is being removed.
        /// Fires `on_lost` in that case so the author can drop wgpu
        /// objects holding a borrow on this view's CAMetalLayer
        /// surface; clears `ready_fired` so a subsequent add re-fires
        /// `on_ready`.
        ///
        /// Caveat: `willMoveToSuperview` only fires when the view's
        /// IMMEDIATE superview changes. When an ancestor (a navigator
        /// screen, a list cell) is removed and the view is detached
        /// via cascade, UIKit does NOT fire this method on
        /// descendants — see `willMoveToWindow:` below for the
        /// cascade-safe trigger.
        #[method(willMoveToSuperview:)]
        fn will_move_to_superview(&self, new_superview: *const objc2_ui_kit::UIView) {
            let _: () = unsafe {
                msg_send![super(self), willMoveToSuperview: new_superview]
            };
            if new_superview.is_null() && self.ivars().ready_fired.get() {
                if let Some(cb) = self.ivars().on_lost.borrow_mut().as_mut() {
                    cb();
                }
                self.ivars().ready_fired.set(false);
                self.ivars().last_size.set((0, 0));
            }
        }

        /// Cascade-safe sibling of `willMoveToSuperview:`. UIKit fires
        /// `willMoveToWindow:` on the entire descendant tree whenever
        /// an ancestor is added or removed from a window — including
        /// the case where a navigator screen far above us is removed
        /// (`MountPolicy::LazyDisposing` releasing the home screen,
        /// a list cell being recycled, etc). Without this hook, a
        /// MetalView buried inside a torn-down screen never receives
        /// the lifecycle event that releases its strong-Rc cycle
        /// through the slot's `on_*` closures — the wgpu host stays
        /// alive (with its render-loop NSTimer firing on a detached
        /// CAMetalLayer) until the WHOLE app dies.
        ///
        /// We only fire when `newWindow` is null AND `ready_fired`
        /// is set, so the inverse transition (added to a window,
        /// first layout fires `on_ready`) doesn't bounce through a
        /// stale `on_lost`.
        #[method(willMoveToWindow:)]
        fn will_move_to_window(&self, new_window: *const objc2_foundation::NSObject) {
            let _: () = unsafe {
                msg_send![super(self), willMoveToWindow: new_window]
            };
            if new_window.is_null() && self.ivars().ready_fired.get() {
                if let Some(cb) = self.ivars().on_lost.borrow_mut().as_mut() {
                    cb();
                }
                self.ivars().ready_fired.set(false);
                self.ivars().last_size.set((0, 0));
            }
        }
    }
);

impl MetalView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(MetalViewIvars {
            on_ready: RefCell::new(None),
            on_resize: RefCell::new(None),
            on_lost: RefCell::new(None),
            surface: RefCell::new(None),
            last_size: Cell::new((0, 0)),
            ready_fired: Cell::new(false),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Install the three framework-supplied callbacks. Called once
    /// from `imp::graphics::create_graphics` after the view is built.
    pub(crate) fn install_callbacks(
        &self,
        on_ready: OnReady,
        on_resize: OnResize,
        on_lost: OnLost,
        surface: GraphicsSurface,
    ) {
        *self.ivars().on_ready.borrow_mut() = Some(on_ready);
        *self.ivars().on_resize.borrow_mut() = Some(on_resize);
        *self.ivars().on_lost.borrow_mut() = Some(on_lost);
        *self.ivars().surface.borrow_mut() = Some(surface);
    }
}

// =========================================================================
// LayoutObserverView — UIView subclass that triggers a layout pass when
// its bounds change.
//
// The host UIView (passed in via `ios_main`) doesn't notify us when its
// bounds change (orientation flip, iPad split-view resize, keyboard
// frame change, etc.). Adding an instance of this subclass as a child
// of the host with `autoresizingMask = .flexibleWidth | .flexibleHeight`
// means UIKit calls `layoutSubviews` on us every time the parent's
// frame changes — exactly the signal we need to re-run Taffy.
//
// We dedupe by remembering the last size we re-laid out at, so the many
// `layoutSubviews` calls UIKit makes during a single stable bounds get
// at most one layout pass.
// =========================================================================

pub(crate) struct LayoutObserverIvars {
    last_size: std::cell::Cell<(f32, f32)>,
}

declare_class!(
    pub(crate) struct LayoutObserverView;

    unsafe impl ClassType for LayoutObserverView {
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystLayoutObserverView";
    }

    impl DeclaredClass for LayoutObserverView {
        type Ivars = LayoutObserverIvars;
    }

    unsafe impl LayoutObserverView {
        #[method(layoutSubviews)]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            let bounds: objc2_foundation::CGRect = unsafe { msg_send![self, bounds] };
            let w = bounds.size.width as f32;
            let h = bounds.size.height as f32;
            let (lw, lh) = self.ivars().last_size.get();
            // Skip the first call (lw == lh == 0 means we've never
            // measured) — the framework's initial render already ran a
            // layout pass at startup, so re-running now would be
            // wasteful. Trigger only on real size changes after that.
            let changed = (lw - w).abs() > 0.5 || (lh - h).abs() > 0.5;
            if changed {
                self.ivars().last_size.set((w, h));
                // Push to the framework's reactive viewport signal so
                // `viewport_size()` subscribers (responsive containers,
                // breakpoint hooks) re-fire. We do this on the FIRST
                // measurement too — unlike the layout-pass kick below
                // — because author code may want the initial value
                // even on the very first frame.
                runtime_core::set_viewport_size(runtime_core::ViewportSize {
                    width: w,
                    height: h,
                });
                if lw != 0.0 || lh != 0.0 {
                    crate::imp::schedule_layout_pass();
                }
            }
        }

        /// UIKit calls this on every view in a chain when the
        /// effective safe-area insets change (rotation, dynamic
        /// island, status-bar hide/show, sheet adaptation). Our
        /// observer sits as a flexible-resize subview of the host
        /// root, so its `safeAreaInsets` mirror the host's — read
        /// them and push to the framework's global signal. Effects
        /// subscribed via `safe_area_insets()` re-fire downstream,
        /// including any container's `apply_safe_area_padding`.
        #[method(safeAreaInsetsDidChange)]
        fn safe_area_insets_did_change(&self) {
            let _: () = unsafe { msg_send![super(self), safeAreaInsetsDidChange] };
            let insets: UIEdgeInsets = unsafe { msg_send![self, safeAreaInsets] };
            runtime_core::set_safe_area_insets(runtime_core::EdgeInsets {
                top: insets.top as f32,
                right: insets.right as f32,
                bottom: insets.bottom as f32,
                left: insets.left as f32,
            });
        }
    }
);

impl LayoutObserverView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(LayoutObserverIvars {
            last_size: std::cell::Cell::new((0.0, 0.0)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// KeyboardObserver — NSObject that observes the soft keyboard's frame and
// drives the framework's viewport bottom-inset so content reflows above the
// keyboard (and restores when it dismisses).
//
// Unlike Android (where the window resizes), UIKit OVERLAYS the soft keyboard
// without changing the host view's bounds — so `LayoutObserverView` above
// never fires for the IME, and the layout doesn't reflow on open OR close. We
// instead observe `UIKeyboardWillChangeFrameNotification` (which covers show,
// hide, and frame/height changes in one), read the keyboard's end frame, and
// hand it to the backend, which computes how much of the host it covers and
// shrinks the layout viewport by that amount.
//
// NSNotificationCenter does NOT retain its `addObserver:selector:…` observers,
// so the backend retains this object in `callback_targets`; dropping that ref
// ends observation. No ivars — the backend is reached via the global
// `with_backend` self-handle (the notification fires on the main thread, the
// only thread the backend is touched from).
// =========================================================================
declare_class!(
    pub(crate) struct KeyboardObserver;

    unsafe impl ClassType for KeyboardObserver {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystKeyboardObserver";
    }

    impl DeclaredClass for KeyboardObserver {
        type Ivars = ();
    }

    unsafe impl KeyboardObserver {
        #[method(keyboardFrameWillChange:)]
        fn keyboard_frame_will_change(&self, note: &NSObject) {
            // `note` is the NSNotification. Pull the keyboard's END frame
            // (window/screen base coordinates) out of
            // `userInfo[UIKeyboardFrameEndUserInfoKey]`.
            if let Some(rect) = unsafe { keyboard_end_frame(note) } {
                crate::imp::with_backend(|b| b.on_keyboard_frame_changed(rect));
            }
        }
    }
);

impl KeyboardObserver {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

/// Extract the keyboard's end frame (window/screen base coordinates) from a
/// `UIKeyboardWillChangeFrameNotification`. Returns `None` if `userInfo` or
/// the frame value is absent. The value under `UIKeyboardFrameEndUserInfoKey`
/// is an `NSValue` wrapping a `CGRect`.
///
/// # Safety
/// `note` must be a valid `NSNotification`.
pub(crate) unsafe fn keyboard_end_frame(note: &NSObject) -> Option<objc2_foundation::CGRect> {
    let user_info: *mut NSObject = msg_send![note, userInfo];
    if user_info.is_null() {
        return None;
    }
    let key = NSString::from_str("UIKeyboardFrameEndUserInfoKey");
    let value: *mut NSObject = msg_send![user_info, objectForKey: &*key];
    if value.is_null() {
        return None;
    }
    let rect: objc2_foundation::CGRect = msg_send![value, CGRectValue];
    Some(rect)
}

// =========================================================================
// OverlayPassthroughView — UIView subclass that only consumes touches
// in its subviews' frames.
//
// Used as the container for portals (formerly: overlays without a backdrop)
// (popovers, selects). The default `UIView.pointInside:` returns YES
// for any point in `bounds`, so a viewport-spanning container would
// intercept every touch on the page beneath it — including scroll
// gestures. Overriding `pointInside:` to return YES only where a
// subview lies makes the container act like an invisible parent that
// "wraps" just the popover content: taps and pan gestures outside the
// content fall through to whatever's behind the overlay (the page),
// while touches inside the content still reach the popover.
// =========================================================================

declare_class!(
    pub(crate) struct OverlayPassthroughView;

    unsafe impl ClassType for OverlayPassthroughView {
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystOverlayPassthroughView";
    }

    impl DeclaredClass for OverlayPassthroughView {
        type Ivars = ();
    }

    unsafe impl OverlayPassthroughView {
        #[method(pointInside:withEvent:)]
        fn point_inside(
            &self,
            point: objc2_foundation::CGPoint,
            _event: *const NSObject,
        ) -> objc2::runtime::Bool {
            // Hit only if the point lies inside one of our subviews.
            // We don't dig recursively — overlay containers have a
            // small, flat subview list (the single content child,
            // plus an optional scrim that's only present when a
            // backdrop is requested, in which case the caller uses a
            // plain UIView, not this class).
            let subviews: Retained<objc2_foundation::NSArray<UIView>> =
                unsafe { msg_send_id![self, subviews] };
            for sub in subviews.iter() {
                if sub.isHidden() {
                    continue;
                }
                let frame: objc2_foundation::CGRect = unsafe { msg_send![&*sub, frame] };
                if point.x >= frame.origin.x
                    && point.x < frame.origin.x + frame.size.width
                    && point.y >= frame.origin.y
                    && point.y < frame.origin.y + frame.size.height
                {
                    return objc2::runtime::Bool::YES;
                }
            }
            objc2::runtime::Bool::NO
        }
    }
);

impl OverlayPassthroughView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// PrivateLayerPassthroughView — the screen_recorder `PrivateLayer` window's
// root content view. Like `OverlayPassthroughView` it lets touches fall
// through to the app window beneath, but its hit-test is RECURSIVE.
//
// The portal `OverlayPassthroughView` checks only its DIRECT subviews'
// frames, because a portal's direct child IS the small popover content.
// The private layer is different: its direct child is a viewport-spanning,
// transparent flex root (the content is a Taffy root sized to the window),
// with the actual controls — a toolbar, a recording preview — nested deep
// inside and sparse. A direct-children-frame check therefore reports YES for
// EVERY point and swallows all canvas-area touches (the user "can't draw at
// all"; the toolbar still works because those taps land in that same
// full-screen child and hit-test down to the buttons).
//
// So we descend the subtree and capture a touch only where it lands on a view
// that actually wants it: an interactive control (an `IdealystTouchView` with
// an installed `on_touch` handler) or a view that paints visible content (a
// non-clear background, alpha > 0). Transparent layout containers are passed
// through, so a tap in the empty regions above/around the toolbar falls
// through this window to the drawable canvas in the app window beneath.
// =========================================================================

extern "C" {
    /// CoreGraphics: alpha component of a `CGColorRef` (0.0 = fully
    /// transparent). Linked from the system CoreGraphics framework.
    fn CGColorGetAlpha(color: *const std::ffi::c_void) -> CGFloat;
}

/// Recursive hit-test for [`PrivateLayerPassthroughView`]. Returns true if
/// `point` (in `view`'s coordinate space) lands on a subview that should
/// CAPTURE the touch — see the type docs for the criteria.
///
/// Builds a [`HitNode`](crate::private_layer_hittest::HitNode) tree from the
/// live UIView subtree (reading each subview's frame + whether it captures)
/// and delegates the recursion + coordinate conversion to the host-tested
/// [`region_blocks_touch`](crate::private_layer_hittest::region_blocks_touch).
/// The private layer's controls use plain frames (no scroll offset / transform),
/// so a subview's frame origin is the exact parent→child coordinate offset.
pub(crate) fn private_layer_blocks_touch(
    view: &UIView,
    point: objc2_foundation::CGPoint,
) -> bool {
    let nodes = private_layer_hit_nodes(view);
    crate::private_layer_hittest::region_blocks_touch(&nodes, point.x, point.y)
}

/// Build the [`HitNode`] tree for `view`'s subviews (recursively): frame in the
/// parent's coordinate space + whether each subview itself captures touches.
/// Hidden subviews are skipped (they can't be hit).
fn private_layer_hit_nodes(view: &UIView) -> Vec<crate::private_layer_hittest::HitNode> {
    let subviews: Retained<objc2_foundation::NSArray<UIView>> =
        unsafe { msg_send_id![view, subviews] };
    let mut nodes = Vec::new();
    for sub in subviews.iter() {
        if sub.isHidden() {
            continue;
        }
        let frame: objc2_foundation::CGRect = unsafe { msg_send![&*sub, frame] };
        nodes.push(crate::private_layer_hittest::HitNode {
            x: frame.origin.x,
            y: frame.origin.y,
            w: frame.size.width,
            h: frame.size.height,
            captures: private_layer_view_captures(&sub),
            children: private_layer_hit_nodes(&sub),
        });
    }
    nodes
}

/// A single view captures touches if it's an interactive control (an
/// `IdealystTouchView` with an installed handler) or paints visible content
/// (a non-clear background, alpha > 0).
fn private_layer_view_captures(view: &UIView) -> bool {
    // Interactive control: IdealystTouchView with a handler installed.
    let touch_cls = objc2::class!(IdealystTouchView);
    let is_touch: bool = unsafe { msg_send![view, isKindOfClass: touch_cls] };
    if is_touch {
        // SAFETY: dynamic class just confirmed IdealystTouchView; its layout
        // is UIView extended with our ivars, ABI-identical for this read.
        let tv: &super::touch::IdealystTouchView =
            unsafe { &*(view as *const UIView as *const super::touch::IdealystTouchView) };
        if tv.has_handler() {
            return true;
        }
    }
    // Visible content: non-clear background (alpha > 0). `backgroundColor` is
    // nil on a plain transparent layout container → falls through.
    let bg: Option<Retained<NSObject>> = unsafe { msg_send_id![view, backgroundColor] };
    if let Some(bg) = bg {
        let cg: *const std::ffi::c_void = unsafe { msg_send![&bg, CGColor] };
        if !cg.is_null() && unsafe { CGColorGetAlpha(cg) } > 0.0 {
            return true;
        }
    }
    false
}

declare_class!(
    pub(crate) struct PrivateLayerPassthroughView;

    unsafe impl ClassType for PrivateLayerPassthroughView {
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystPrivateLayerPassthroughView";
    }

    impl DeclaredClass for PrivateLayerPassthroughView {
        type Ivars = ();
    }

    unsafe impl PrivateLayerPassthroughView {
        #[method(pointInside:withEvent:)]
        fn point_inside(
            &self,
            point: objc2_foundation::CGPoint,
            _event: *const NSObject,
        ) -> objc2::runtime::Bool {
            // SAFETY: PrivateLayerPassthroughView's superclass is UIView.
            let this: &UIView = unsafe { &*(self as *const Self as *const UIView) };
            if private_layer_blocks_touch(this, point) {
                objc2::runtime::Bool::YES
            } else {
                objc2::runtime::Bool::NO
            }
        }
    }
);

impl PrivateLayerPassthroughView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(());
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// PassthroughWindow — the screen_recorder `PrivateLayer`'s separate UIWindow.
//
// `PrivateLayerPassthroughView.pointInside:` returning NO makes the ROOT
// view's `hitTest` return nil over the canvas — but that is NOT enough to let
// the touch fall through to the app window beneath. A `UIWindow`'s own
// `pointInside:` is YES across its whole (screen-sized) bounds, so when its
// root view declines, the default `hitTest` returns the WINDOW ITSELF — a
// non-nil result — and UIKit delivers the touch to this overlay window, where
// it dies (the drawing surface in the app window never sees it; confirmed by
// logging: 0 of 20 canvas touches reached `touchesBegan`).
//
// Overriding the window's `hitTest:` to return nil when the resolved view is
// the window itself makes the whole window decline those touches, so UIKit
// proceeds to the next (key/app) window — the canvas becomes drawable while
// the toolbar (a real subview, hit non-self) still captures.
declare_class!(
    pub(crate) struct PassthroughWindow;

    unsafe impl ClassType for PassthroughWindow {
        type Super = UIWindow;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystPassthroughWindow";
    }

    impl DeclaredClass for PassthroughWindow {
        type Ivars = ();
    }

    unsafe impl PassthroughWindow {
        #[method_id(hitTest:withEvent:)]
        fn hit_test(
            &self,
            point: objc2_foundation::CGPoint,
            event: *const NSObject,
        ) -> Option<Retained<UIView>> {
            let hit: Option<Retained<UIView>> =
                unsafe { msg_send_id![super(self), hitTest: point, withEvent: event] };
            // Resolved to the window itself → no private-layer control was hit
            // (the root view's recursive `pointInside` declined). Pass through
            // (nil) so the app window beneath receives the touch. A real
            // control resolves to a deep subview (not self) and is returned.
            match hit {
                Some(v)
                    if (Retained::as_ptr(&v) as *const ())
                        == (self as *const Self as *const ()) =>
                {
                    None
                }
                other => other,
            }
        }
    }
);

impl PassthroughWindow {
    pub(crate) fn new_with_frame(
        mtm: MainThreadMarker,
        frame: objc2_foundation::CGRect,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(());
        unsafe { msg_send_id![super(this), initWithFrame: frame] }
    }
}

// =========================================================================
// DisplayLinkTarget — CADisplayLink target that calls a Rust closure
// once per display refresh. Same shape as `CallbackTarget` but the
// selector accepts the display-link sender (which we ignore).
//
// Used by the overlay primitive to track an element-anchored overlay
// to its trigger as the page scrolls / animates. Cheaper than
// observing every potential UIScrollView ancestor and re-runs every
// vsync only while the link is added to a runloop — invalidating it
// stops all work.
// =========================================================================

pub(crate) struct DisplayLinkTargetIvars {
    callback: RefCell<Option<Rc<dyn Fn()>>>,
}

declare_class!(
    pub(crate) struct DisplayLinkTarget;

    unsafe impl ClassType for DisplayLinkTarget {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystDisplayLinkTarget";
    }

    impl DeclaredClass for DisplayLinkTarget {
        type Ivars = DisplayLinkTargetIvars;
    }

    unsafe impl DisplayLinkTarget {
        #[method(tick:)]
        fn tick(&self, _sender: &NSObject) {
            let ivars = self.ivars();
            if let Some(cb) = ivars.callback.borrow().as_ref() {
                cb();
            }
        }
    }
);

impl DisplayLinkTarget {
    pub(crate) fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn()>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(DisplayLinkTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// TextKeyDelegate — UITextFieldDelegate + UITextViewDelegate bridge
// =========================================================================
//
// Single delegate object that handles BOTH UIKit text widgets. The
// keydown bridge uses `shouldChangeText` (the universal UIKit hook
// that fires for every typed character, paste, and named-key insert
// like Tab/Enter/Backspace) — returning `false` suppresses the
// change, which maps cleanly to `KeyOutcome::PreventDefault`.
//
// For UITextView we ALSO carry the on_change closure here, because
// UITextView reports value changes via `textViewDidChange:` on the
// delegate rather than the target/action pattern UITextField uses.
// UITextField's on_change continues to ride target/action with the
// existing `StringCallbackTarget`; the delegate slot on that widget
// only carries the optional key handler.
//
// `key` and `on_change` are both Option so the same class works for
// either widget — UITextField sets only `key`, UITextView sets both.

pub(crate) struct TextKeyDelegateIvars {
    pub(crate) key: RefCell<Option<KeyDownHandler>>,
    pub(crate) on_change: RefCell<Option<Rc<dyn Fn(String)>>>,
}

declare_class!(
    pub(crate) struct TextKeyDelegate;

    unsafe impl ClassType for TextKeyDelegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystTextKeyDelegate";
    }

    impl DeclaredClass for TextKeyDelegate {
        type Ivars = TextKeyDelegateIvars;
    }

    unsafe impl TextKeyDelegate {
        /// `UITextFieldDelegate.textField:shouldChangeCharactersInRange:replacementString:`
        /// — UIKit's pre-default-action hook for UITextField input.
        /// Fires once per typed character (incl. Tab, Backspace,
        /// Enter), once per pasted blob, etc.
        #[method(textField:shouldChangeCharactersInRange:replacementString:)]
        fn text_field_should_change(
            &self,
            _text_field: &UITextField,
            range: NSRange,
            string: &NSString,
        ) -> bool {
            self.dispatch_key(range, string)
        }

        /// `UITextViewDelegate.textView:shouldChangeTextInRange:replacementText:`
        /// — identical contract on the multi-line widget.
        #[method(textView:shouldChangeTextInRange:replacementText:)]
        fn text_view_should_change(
            &self,
            _text_view: &UITextView,
            range: NSRange,
            text: &NSString,
        ) -> bool {
            self.dispatch_key(range, text)
        }

        /// `UITextViewDelegate.textViewDidChange:` — fires after the
        /// content actually changes (post-shouldChangeText accept).
        /// UITextView has no target/action equivalent, so on_change
        /// rides this method.
        #[method(textViewDidChange:)]
        fn text_view_did_change(&self, text_view: &UITextView) {
            let ivars = self.ivars();
            if let Some(cb) = ivars.on_change.borrow().as_ref() {
                let text: Option<Retained<NSString>> =
                    unsafe { msg_send_id![text_view, text] };
                let s = text.map(|ns| ns.to_string()).unwrap_or_default();
                cb(s);
            }
        }
    }
);

impl TextKeyDelegate {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        key: Option<KeyDownHandler>,
        on_change: Option<Rc<dyn Fn(String)>>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(TextKeyDelegateIvars {
            key: RefCell::new(key),
            on_change: RefCell::new(on_change),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Map the (range, replacement) tuple UIKit hands us into our
    /// canonical [`KeyEvent`] shape, invoke the user handler, and
    /// translate [`KeyOutcome`] into the BOOL UIKit expects.
    ///
    /// Replacement-text → `key` heuristics — chosen to match the
    /// vocabulary documented on `runtime_core::primitives::key`:
    ///
    /// - `""` with `range.length > 0` → `"Backspace"`. (UIKit reports
    ///   backspace as a deletion of the character behind the caret;
    ///   no other Apple-platform code path fires shouldChangeText
    ///   with empty replacement.)
    /// - `"\t"` → `"Tab"`.
    /// - `"\n"` → `"Enter"`.
    /// - single character → the character itself ("a", "A", " ").
    /// - longer string → first char, mirroring browser keydown
    ///   semantics for IME composition / paste-as-single-key.
    fn dispatch_key(&self, range: NSRange, replacement: &NSString) -> bool {
        let ivars = self.ivars();
        let handler = match ivars.key.borrow().as_ref() {
            Some(h) => h.clone(),
            None => return true,
        };
        let text = replacement.to_string();
        let key = if text.is_empty() {
            "Backspace".to_string()
        } else if text == "\t" {
            "Tab".to_string()
        } else if text == "\n" {
            "Enter".to_string()
        } else if let Some(c) = text.chars().next() {
            c.to_string()
        } else {
            String::new()
        };
        let event = KeyEvent {
            key,
            // UIKit doesn't surface modifier state through
            // shouldChangeText. For text-editor use cases (Tab to
            // indent, etc.) the modifier doesn't matter; richer
            // modifier reads would need UIPress / pressesBegan
            // tracking layered on top.
            shift: false,
            ctrl: false,
            alt: false,
            meta: false,
            selection_start: range.location,
            selection_end: range.location + range.length,
        };
        match handler(&event) {
            KeyOutcome::PreventDefault => false,
            KeyOutcome::Default => true,
        }
    }
}

// =========================================================================
// ScrollDelegate — UIScrollViewDelegate bridge for `on_scroll`
// =========================================================================
//
// Bridges `UIScrollViewDelegate::scrollViewDidScroll:` into a Rust
// closure. The framework's `create_scroll_view` retains one delegate
// per scroll view; UIKit calls `scrollViewDidScroll:` on every
// `contentOffset` change (touch-driven, programmatic, or rubber-band
// settle).

pub(crate) struct ScrollDelegateIvars {
    callback: RefCell<Option<Rc<dyn Fn(f32, f32)>>>,
}

declare_class!(
    pub(crate) struct ScrollDelegate;

    unsafe impl ClassType for ScrollDelegate {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystScrollDelegate";
    }

    impl DeclaredClass for ScrollDelegate {
        type Ivars = ScrollDelegateIvars;
    }

    unsafe impl ScrollDelegate {
        #[method(scrollViewDidScroll:)]
        fn scroll_view_did_scroll(&self, scroll_view: &UIScrollView) {
            let ivars = self.ivars();
            if let Some(cb) = ivars.callback.borrow().as_ref() {
                let offset: objc2_foundation::CGPoint =
                    unsafe { msg_send![scroll_view, contentOffset] };
                cb(offset.x as f32, offset.y as f32);
            }
        }
    }
);

impl ScrollDelegate {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        callback: Rc<dyn Fn(f32, f32)>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ScrollDelegateIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::{tap_gate_decision, TAP_GATE_SETTLE_SECS};

    // Regression: Link/Pressable on iOS fired `on_click` at mount.
    // UIKit delivered a phantom tap in the same run-loop turn the view
    // entered the window (QuillEMR Schedule auto-opened the appt detail
    // modal with no user tap). With `window_entry` stamped at CREATION
    // (≈ on-demand mount), the phantom — which fires within the same
    // run-loop turn, ≪ TAP_GATE_SETTLE_SECS after create — is rejected.
    #[test]
    fn regression_ios_pressable_tap_rejected_at_mount() {
        let create = 100.0;
        // Phantom tap, same run-loop turn as mount → well within the settle
        // window of create-time → rejected.
        let (allow, entry) = tap_gate_decision(create, create + 0.01);
        assert!(!allow, "the mount-time phantom tap must be gated");
        assert_eq!(entry, create, "stamp stays at create-time");
    }

    // Regression: the user's FIRST real tap on a pushed screen with NO phantom
    // mount-tap (e.g. the whiteboard Settings: SegmentedControl/Switch) used to be
    // eaten — the OLD gate stamped on the first should-begin call and rejected it,
    // assuming a phantom always arrives first. Stamping at CREATE-time instead
    // means the first real human tap (well past the settle window) is allowed on
    // the first try.
    #[test]
    fn regression_first_real_tap_on_pushed_screen_is_allowed() {
        let create = 100.0;
        let first_real_tap = create + 1.0; // human reaction, past settle
        let (allow, entry) = tap_gate_decision(create, first_real_tap);
        assert!(allow, "first real tap on a no-phantom screen must fire");
        assert_eq!(entry, create);
    }

    #[test]
    fn tap_within_settle_window_is_rejected() {
        // A tap arriving before the settle window elapses is still the
        // synthetic-burst tail; reject it. Entry stays put.
        let now = 100.0 + TAP_GATE_SETTLE_SECS - 0.01;
        let (allow, entry) = tap_gate_decision(100.0, now);
        assert!(!allow);
        assert_eq!(entry, 100.0, "stamp is not moved by later calls");
    }

    #[test]
    fn real_tap_after_settle_window_is_allowed() {
        // A genuine user tap (the screen has rendered and the human has
        // reacted — well past the settle window) must pass through.
        let now = 100.0 + TAP_GATE_SETTLE_SECS + 0.5;
        let (allow, entry) = tap_gate_decision(100.0, now);
        assert!(allow, "a real post-mount tap must fire on_click");
        assert_eq!(entry, 100.0);
    }

    #[test]
    fn tap_exactly_at_settle_boundary_is_allowed() {
        // Anchor entry at 0.0 so `now - entry` is EXACTLY TAP_GATE_SETTLE_SECS.
        // Using a large base (e.g. 100.0 + 0.35 - 100.0) suffers catastrophic
        // float cancellation and lands a hair below the threshold, which would
        // spuriously fail the inclusive (`>=`) boundary it's meant to assert.
        let (allow, _) = tap_gate_decision(0.0, TAP_GATE_SETTLE_SECS);
        assert!(allow, "boundary is inclusive (>=)");
    }
}
