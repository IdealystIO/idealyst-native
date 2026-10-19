use runtime_shared::primitives::key::{KeyDownHandler, KeyEvent, KeyOutcome};
use runtime_shared::primitives::scroll_view::EndReach;
use runtime_shared::primitives::text_input::{BlurHandler, BlurOutcome};
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

/// Whether a touch may drive a Link/Pressable tap recognizer: only if it
/// BEGAN after the control was created. `touch_began` is the touch's
/// `UITouch.timestamp` as UIKit hands it to `gestureRecognizer:
/// shouldReceiveTouch:` (the touch-down time), `created` the
/// `CACurrentMediaTime()` stamped in [`CallbackTarget::new`]; both read the
/// same clock (seconds of host uptime).
///
/// This replaces a 0.35 s "settle" window after creation, which existed to
/// stop a phantom tap UIKit delivered to a freshly-mounted view (QuillEMR's
/// Schedule auto-opened a modal with no user tap; the cause was never
/// pinned down). A window can't tell that phantom from a real tap: any
/// control REBUILT by a press (a Button whose live `disabled`/`loading`
/// flips — a pin pad's backspace the moment the first digit lands) refused
/// every tap for 0.35 s, so fast entry was silently dropped. A touch that
/// began before the control existed, though, cannot be a press of it: the
/// finger landed on something else. That is the line this draws, and the
/// one the mount-time phantom crosses if it is the in-flight touch that
/// caused the mount. A human tap on the control always begins after it was
/// created, however soon after.
fn touch_may_tap(touch_began: f64, created: f64) -> bool {
    touch_began > created
}

pub struct CallbackTargetIvars {
    callback: RefCell<Option<Rc<dyn Fn()>>>,
    /// `CACurrentMediaTime()` when this target (and so its control) was
    /// created. A touch that began earlier is refused — see
    /// [`touch_may_tap`].
    created: f64,
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

        /// `UIGestureRecognizerDelegate.gestureRecognizer:shouldReceiveTouch:`.
        /// Refuses a touch that began before this control was created —
        /// see [`touch_may_tap`]. Asked once per new touch, at touch-down,
        /// so `timestamp` is when the finger landed.
        #[method(gestureRecognizer:shouldReceiveTouch:)]
        fn gesture_recognizer_should_receive_touch(
            &self,
            _recognizer: &objc2_ui_kit::UIGestureRecognizer,
            touch: &objc2::runtime::AnyObject,
        ) -> objc2::runtime::Bool {
            let began: f64 = unsafe { msg_send![touch, timestamp] };
            objc2::runtime::Bool::new(touch_may_tap(began, self.ivars().created))
        }

        /// `UIGestureRecognizerDelegate.gestureRecognizerShouldBegin:`.
        /// A recognizer whose view is not in a window cannot have been
        /// tapped by a person. Recognizers we don't wire as Link/Pressable
        /// taps never reach here (we only set this object as their
        /// delegate).
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
            objc2::runtime::Bool::new(!window.is_null())
        }
    }
);

impl CallbackTarget {
    pub fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn()>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        // The control is built immediately before it is mounted, so
        // creation is the earliest moment a person could touch it.
        let created = unsafe { CACurrentMediaTime() };
        let this = this.set_ivars(CallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
            created,
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
// `websites/website/src/components/simulator.rs`).

use std::cell::Cell;
use runtime_shared::primitives::graphics::{
    GraphicsSurface, GraphicsTarget, OnLost, OnReady, OnReadyEvent, OnResize, OnResizeEvent,
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
                        target: GraphicsTarget::RawWindow(surface),
                        size: new_size,
                        // `size` is frame × contentScaleFactor, so this is the
                        // physical-px-per-point ratio a renderer divides by to
                        // get the LOGICAL size its author content (and touch
                        // positions, which UIKit reports in points) lives in.
                        // Reporting 1.0 here made GPU views treat physical px
                        // as points: content drew ~3× small and picks missed.
                        scale: scale as f32,
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
                cb(OnResizeEvent { size: new_size, scale: scale as f32 });
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
            // Drain any queued Taffy pass HERE, in UIKit's layout phase.
            //
            // This is the only drain point that precedes every commit. The
            // pre-commit runloop observer misses a commit made from inside
            // the commit phase, and the dispatch backstop is a turn late —
            // between them a freshly built subtree could be painted with no
            // layout at all (children stacked at their parent's origin,
            // which is what the stranded-glyph artifact was). UIKit always
            // lays out before it commits, so draining here cannot be
            // outrun. `schedule_layout_pass` marks this view as needing
            // layout to bring us here.
            //
            // Ordering matters: this runs BEFORE the viewport-change work
            // below, so a pass queued by ordinary reactivity is flushed on
            // its own terms and a genuine resize still queues a fresh one.
            crate::imp::drain_queued_layout_pass();
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
                runtime_shared::set_viewport_size(runtime_shared::ViewportSize {
                    width: w,
                    height: h,
                });
                // Forward the SAME size into the mounted world's
                // viewport ctx so breakpoint-dependent author
                // reactivity re-fires on rotation/resize (no-op TLS
                // read before boot). See
                // newcore.rs, "Viewport source".
                crate::newcore::forward_viewport(runtime_shared::ViewportSize {
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
        /// them and push them to the mounted world, where author
        /// `safe_area_insets()` readers and every `safe_area`
        /// container's re-apply effect subscribe.
        ///
        /// This is also how the FIRST real insets arrive. The tree
        /// mounts from `viewDidLoad`, before the host view is in a
        /// window, and a view outside a window reports zero insets;
        /// the real ones land here once UIKit attaches it. Without the
        /// world push, every `safe_area` padding stayed at that zero
        /// (an iPad header drawn under the status bar).
        #[method(safeAreaInsetsDidChange)]
        fn safe_area_insets_did_change(&self) {
            let _: () = unsafe { msg_send![super(self), safeAreaInsetsDidChange] };
            let insets: UIEdgeInsets = unsafe { msg_send![self, safeAreaInsets] };
            let insets = runtime_shared::EdgeInsets {
                top: insets.top as f32,
                right: insets.right as f32,
                bottom: insets.bottom as f32,
                left: insets.left as f32,
            };
            // The runtime-server dev client resolves wire safe-area
            // opt-ins against the shared value; the world source is
            // the local-render one.
            runtime_shared::set_safe_area_insets(insets);
            crate::newcore::forward_safe_area(insets);
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
// ScreenBoxObserverView — the `LayoutObserverView` of a view controller
// that `mount_screen_in_vc` hosts a screen in.
//
// The layout pass computes such a screen against its controller view's
// bounds, and UIKit sizes that view on its own schedule (the navigation
// controller's first layout, rotation, a parent's `safe_area` padding
// arriving after mount) — none of which reaches the host's observer when
// the window size is unchanged. Same flexible-resize-subview trick.
//
// Unlike the host's observer it does NOT skip its first measurement: a
// pushed screen's controller view is usually sized by UIKit AFTER the
// mount's layout pass ran against a fallback box, so the first real size
// is exactly the one that must re-run it. Nor does it touch the viewport
// or safe-area signals — those describe the window, not this screen.
// =========================================================================

declare_class!(
    pub(crate) struct ScreenBoxObserverView;

    unsafe impl ClassType for ScreenBoxObserverView {
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystScreenBoxObserverView";
    }

    impl DeclaredClass for ScreenBoxObserverView {
        type Ivars = LayoutObserverIvars;
    }

    unsafe impl ScreenBoxObserverView {
        #[method(layoutSubviews)]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            let bounds: objc2_foundation::CGRect = unsafe { msg_send![self, bounds] };
            let w = bounds.size.width as f32;
            let h = bounds.size.height as f32;
            let (lw, lh) = self.ivars().last_size.get();
            if (lw - w).abs() <= 0.5 && (lh - h).abs() <= 0.5 {
                return;
            }
            self.ivars().last_size.set((w, h));
            // Queue, then drain here in UIKit's layout phase so the
            // screen's content is re-framed before this commit — a
            // push animation would otherwise show a frame of content
            // laid out at the old box. Same drain point and borrow
            // handling as the host's observer.
            crate::imp::schedule_layout_pass();
            crate::imp::drain_queued_layout_pass();
        }
    }
);

impl ScreenBoxObserverView {
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
// self-handle (the notification fires on the main thread, the only thread the
// backend is touched from) through `deliver_keyboard_frame`, which never drops
// a frame: if the backend is borrowed the frame waits in a mailbox that the
// next layout pass drains.
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
            //
            // NOT `with_backend`: that drops the call when the backend is
            // borrowed, and the CLOSE frame typically arrives exactly then
            // (a flush unmounting the focused field makes UIKit resign first
            // responder and post this synchronously). See
            // `keyboard_frame_policy`.
            if let Some(change) = unsafe { keyboard_frame_change(note) } {
                crate::imp::deliver_keyboard_frame(change);
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

/// Extract the keyboard's end frame (window/screen base coordinates) and
/// the animation that moves it there from a
/// `UIKeyboardWillChangeFrameNotification`. Returns `None` if `userInfo` or
/// the frame value is absent. The value under `UIKeyboardFrameEndUserInfoKey`
/// is an `NSValue` wrapping a `CGRect`; duration and curve are `NSNumber`s
/// (absent → no animation, which applies the frame immediately).
///
/// # Safety
/// `note` must be a valid `NSNotification`.
pub(crate) unsafe fn keyboard_frame_change(
    note: &NSObject,
) -> Option<crate::keyboard_frame_policy::KeyboardFrameChange<objc2_foundation::CGRect>> {
    let user_info: *mut NSObject = msg_send![note, userInfo];
    if user_info.is_null() {
        return None;
    }
    let lookup = |name: &str| -> *mut NSObject {
        let key = NSString::from_str(name);
        msg_send![user_info, objectForKey: &*key]
    };
    let value = lookup("UIKeyboardFrameEndUserInfoKey");
    if value.is_null() {
        return None;
    }
    let frame: objc2_foundation::CGRect = msg_send![value, CGRectValue];
    let duration = lookup("UIKeyboardAnimationDurationUserInfoKey");
    let duration_s: f64 = if duration.is_null() { 0.0 } else { msg_send![duration, doubleValue] };
    let curve = lookup("UIKeyboardAnimationCurveUserInfoKey");
    let curve: isize = if curve.is_null() { 0 } else { msg_send![curve, integerValue] };
    Some(crate::keyboard_frame_policy::KeyboardFrameChange { frame, duration_s, curve })
}

// =========================================================================
// OverlayPassthroughView — UIView subclass that only consumes touches
// in its subviews' frames.
//
// Used as the container for portals (popovers, selects). The default `UIView.pointInside:` returns YES
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
        // The portal's own `StyleRules::pointer_events`, written by
        // `apply_style`. See `OverlayPassthroughView::set_pointer_events`.
        type Ivars = std::cell::Cell<Option<runtime_shared::PointerEvents>>;
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

        // `pointer-events: none` on the portal ROOT — what
        // `overlay().click_through(true)` lowers to. `pointInside:` above
        // only drops the container's EMPTY area; a content child that
        // spans a band (ToastHost's full-width strip, 56pt tall even with
        // no toast showing) still resolved as the hit and swallowed every
        // tap in that band, because the `none` sat here and nothing read
        // it. Apply the same nearest-explicit-wins walk as
        // `IdealystTouchView`, with this container as the `none` root, so
        // only descendants that opt back in with `Auto` (the toast cards)
        // keep their hits. Single tail expression: `declare_class!` wraps
        // `#[method_id]` bodies.
        #[method_id(hitTest:withEvent:)]
        fn hit_test(
            &self,
            point: objc2_foundation::CGPoint,
            event: Option<&objc2_ui_kit::UIEvent>,
        ) -> Option<Retained<UIView>> {
            let result: Option<Retained<UIView>> =
                unsafe { msg_send_id![super(self), hitTest: point, withEvent: event] };
            if matches!(self.ivars().get(), Some(runtime_shared::PointerEvents::None)) {
                result.and_then(|r| crate::imp::touch::resolve_in_none_root(self, r))
            } else {
                result
            }
        }
    }
);

impl OverlayPassthroughView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(std::cell::Cell::new(None));
        unsafe { msg_send_id![super(this), init] }
    }

    /// Record the portal's explicit `pointer_events` (`None` = unset,
    /// default hit-testing). A portal container is the node
    /// `create_portal` hands the framework, so the portal prim's `style`
    /// is applied HERE rather than to an `IdealystTouchView`; without
    /// this slot a click-through overlay's `none` was dropped.
    pub(crate) fn set_pointer_events(&self, v: Option<runtime_shared::PointerEvents>) {
        self.ivars().set(v);
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
    /// Cancelable blur (see [`runtime_shared::primitives::text_input::BlurOutcome`]).
    /// Consulted by `textFieldShouldEndEditing:` — the native veto point that
    /// `endEditing:` (our outside-tap dismiss) honors.
    pub(crate) on_blur: RefCell<Option<BlurHandler>>,
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

        /// `UITextFieldDelegate.textFieldShouldEndEditing:` — UIKit's native
        /// blur veto. Returning `NO` keeps the field editing (focus + keyboard
        /// stay), which `[view endEditing:]` (our outside-tap dismiss) respects.
        #[method(textFieldShouldEndEditing:)]
        fn text_field_should_end_editing(&self, _text_field: &UITextField) -> bool {
            self.should_end_editing()
        }

        /// Same veto for the multi-line widget.
        #[method(textViewShouldEndEditing:)]
        fn text_view_should_end_editing(&self, _text_view: &UITextView) -> bool {
            self.should_end_editing()
        }
    }
);

impl TextKeyDelegate {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        key: Option<KeyDownHandler>,
        on_change: Option<Rc<dyn Fn(String)>>,
        on_blur: Option<BlurHandler>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(TextKeyDelegateIvars {
            key: RefCell::new(key),
            on_change: RefCell::new(on_change),
            on_blur: RefCell::new(on_blur),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Whether the field may end editing now. `Keep` → `false` (veto); no
    /// handler → `true` (allow). Cloned out of the `RefCell` before calling so
    /// the closure can't reentrantly re-borrow it.
    fn should_end_editing(&self) -> bool {
        let cb = self.ivars().on_blur.borrow().clone();
        match cb {
            Some(cb) => cb() != BlurOutcome::Keep,
            None => true,
        }
    }

    /// Map the (range, replacement) tuple UIKit hands us into our
    /// canonical [`KeyEvent`] shape, invoke the user handler, and
    /// translate [`KeyOutcome`] into the BOOL UIKit expects.
    ///
    /// Replacement-text → `key` heuristics — chosen to match the
    /// vocabulary documented on `runtime_shared::primitives::key`:
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
// KeyboardDismissTarget — tap-to-dismiss for blur-on-outside parity
// =========================================================================
//
// Target + `UIGestureRecognizerDelegate` for a `UITapGestureRecognizer` on the
// host root. UIKit leaves the keyboard up when you tap outside a field (unlike
// web/macOS where clicking outside blurs); this brings iOS to parity: a tap on
// blank space (or any non-text-input view) calls `[view endEditing:YES]`,
// resigning the active field. The field's `on_blur` veto
// (`textFieldShouldEndEditing:`) still applies. `shouldReceiveTouch:` skips
// taps landing on a text input so field-to-field focus transfer stays a clean
// native handoff instead of a dismiss+refocus flicker.

/// Sentinel `UIView.tag` marking a focus-preserving press region
/// (`InputOps::mark_preserves_focus`): touches landing inside a view (or
/// descendant of one) carrying this tag are skipped by the
/// keyboard-dismiss tap recognizer, so pressing e.g. an option row in a
/// combobox's anchored menu doesn't `endEditing:` the input the menu
/// belongs to. The iOS half of web's canceled-`pointerdown` focus
/// preservation.
///
/// Why `tag`: framework views are plain `UIView`s (no Idealyst subclass to
/// hold an ivar), and `tag` is otherwise unused by this backend — it
/// defaults to 0 on every view we create, so a nonzero sentinel is an
/// unambiguous marker. Invariant: nothing else in this backend may write
/// `tag` (grep `setTag` before introducing a second use; switch to objc
/// associated objects if one ever appears).
pub(crate) const PRESERVES_FOCUS_TAG: isize = 0x1DEA_F0C5;

declare_class!(
    pub(crate) struct KeyboardDismissTarget;

    unsafe impl ClassType for KeyboardDismissTarget {
        type Super = NSObject;
        type Mutability = mutability::InteriorMutable;
        const NAME: &'static str = "IdealystKeyboardDismissTarget";
    }

    impl DeclaredClass for KeyboardDismissTarget {
        type Ivars = ();
    }

    unsafe impl KeyboardDismissTarget {
        /// Gesture action — end editing on the recognizer's view (the host
        /// root), blurring whatever field is first responder.
        #[method(dismiss:)]
        fn dismiss(&self, gesture: &NSObject) {
            let view: *mut NSObject = unsafe { msg_send![gesture, view] };
            if !view.is_null() {
                // `endEditing:` returns BOOL — type it or objc2 aborts.
                let _: bool = unsafe { msg_send![view, endEditing: true] };
            }
        }

        /// Skip touches that land on a text input — those manage focus
        /// natively (tapping another field transfers focus) — and touches
        /// inside a focus-preserving subtree ([`PRESERVES_FOCUS_TAG`]): a
        /// combobox's anchored option menu or an input adornment must be
        /// tappable without the tap blurring the field it belongs to (a
        /// close-on-blur would unmount the row mid-tap). Everything else
        /// (blank space, buttons) dismisses, matching web's "any outside
        /// click blurs".
        #[method(gestureRecognizer:shouldReceiveTouch:)]
        fn should_receive_touch(&self, _gr: &NSObject, touch: &NSObject) -> objc2::runtime::Bool {
            let view: *mut NSObject = unsafe { msg_send![touch, view] };
            if view.is_null() {
                return objc2::runtime::Bool::new(true);
            }
            for name in ["UITextField", "UITextView"] {
                if let Some(cls) = objc2::runtime::AnyClass::get(name) {
                    let is: bool = unsafe { msg_send![view, isKindOfClass: cls] };
                    if is {
                        return objc2::runtime::Bool::new(false);
                    }
                }
            }
            // Ancestor walk from the touched view: the sentinel tag usually
            // sits on a wrapping container (the menu panel), while the touch
            // is delivered to the deepest hit-test view (a row / label).
            let mut cur = view;
            while !cur.is_null() {
                let tag: isize = unsafe { msg_send![cur, tag] };
                if tag == PRESERVES_FOCUS_TAG {
                    return objc2::runtime::Bool::new(false);
                }
                cur = unsafe { msg_send![cur, superview] };
            }
            objc2::runtime::Bool::new(true)
        }
    }
);

impl KeyboardDismissTarget {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(());
        unsafe { msg_send_id![super(this), init] }
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
    /// The end-of-scroll observer, when one has been installed: the
    /// axis, the arrival rule, and what to call on arrival.
    ///
    /// It rides the SAME delegate as `callback` because a UIScrollView
    /// has exactly one, and a second `setDelegate:` would silently
    /// unhook the first — a scroller with both an `on_scroll` and an
    /// `on_end_reached` would lose whichever was installed first.
    end: RefCell<Option<EndObserver>>,
}

pub(crate) struct EndObserver {
    pub(crate) horizontal: bool,
    pub(crate) reach: RefCell<EndReach>,
    pub(crate) on_end: Rc<dyn Fn()>,
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
            let offset: objc2_foundation::CGPoint =
                unsafe { msg_send![scroll_view, contentOffset] };
            if let Some(cb) = ivars.callback.borrow().as_ref() {
                cb(offset.x as f32, offset.y as f32);
            }
            // The three numbers an app cannot get for itself: where the
            // reader is, how much they can see, and how much there is.
            let fire = {
                let end = ivars.end.borrow();
                match end.as_ref() {
                    None => None,
                    Some(obs) => {
                        let bounds: objc2_foundation::CGRect =
                            unsafe { msg_send![scroll_view, bounds] };
                        let content: objc2_foundation::CGSize =
                            unsafe { msg_send![scroll_view, contentSize] };
                        let (o, v, c) = if obs.horizontal {
                            (offset.x, bounds.size.width, content.width)
                        } else {
                            (offset.y, bounds.size.height, content.height)
                        };
                        obs.reach
                            .borrow_mut()
                            .update(o as f32, v as f32, c as f32)
                            .then(|| obs.on_end.clone())
                    }
                }
            };
            // Called with NO borrow held: the callback loads a page,
            // which re-enters the runtime and can reach this delegate
            // again through a relayout.
            if let Some(on_end) = fire {
                on_end();
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
            end: RefCell::new(None),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// A delegate that only watches for the end — for a scroller with
    /// an `on_end_reached` and no `on_scroll`.
    pub(crate) fn new_watcher(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ScrollDelegateIvars {
            callback: RefCell::new(None),
            end: RefCell::new(None),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Install (or replace) the end-of-scroll observer.
    pub(crate) fn set_end(&self, horizontal: bool, threshold: f32, on_end: Rc<dyn Fn()>) {
        *self.ivars().end.borrow_mut() = Some(EndObserver {
            horizontal,
            reach: RefCell::new(EndReach::new(threshold)),
            on_end,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::touch_may_tap;

    // REGRESSION (kiosk pin pad): a control rebuilt by
    // a press refused every tap for 0.35 s after its creation, so a fast
    // ⌫ right after a digit (the press that flips its live `disabled` and
    // rebuilds it) was silently dropped. Reproduced in the iOS simulator
    // with real HID taps: a Back tap 150 ms and 250 ms after a digit never
    // fired, 600 ms did. A touch that begins after the rebuild — however
    // soon — must be allowed. (A fixed post-creation window rejects all three.)
    #[test]
    fn regression_tap_soon_after_a_rebuild_is_allowed() {
        let created = 100.0;
        assert!(touch_may_tap(created + 0.15, created), "150 ms after the rebuild");
        assert!(touch_may_tap(created + 0.25, created), "250 ms after the rebuild");
        assert!(touch_may_tap(created + 0.01, created), "10 ms after: two thumbs");
    }

    // The mount-time guard the window existed for: a touch already in
    // flight when the control was created landed on something else, so
    // it can never tap this control.
    #[test]
    fn touch_that_began_before_the_control_existed_is_refused() {
        let created = 100.0;
        assert!(!touch_may_tap(created - 0.2, created), "the press that caused the mount");
        assert!(!touch_may_tap(created, created), "same instant is not after");
    }

    // The first real tap on a freshly pushed screen fires on the first try.
    #[test]
    fn first_tap_on_a_pushed_screen_is_allowed() {
        assert!(touch_may_tap(101.0, 100.0));
    }
}
