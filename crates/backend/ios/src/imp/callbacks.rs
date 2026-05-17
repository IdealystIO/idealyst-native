use objc2::rc::Retained;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::UIView;
use std::cell::RefCell;
use std::rc::Rc;

// =========================================================================
// CallbackTarget — ObjC action target that calls a Rust closure
// =========================================================================

pub(crate) struct CallbackTargetIvars {
    callback: RefCell<Option<Rc<dyn Fn()>>>,
}

declare_class!(
    pub(crate) struct CallbackTarget;

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
            let ivars = self.ivars();
            if let Some(cb) = ivars.callback.borrow().as_ref() {
                cb();
            }
        }
    }
);

impl CallbackTarget {
    pub(crate) fn new(mtm: MainThreadMarker, callback: Rc<dyn Fn()>) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(CallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
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
            let ivars = self.ivars();
            if let Some(cb) = ivars.callback.borrow().as_ref() {
                let is_on: bool = unsafe { msg_send![sender, isOn] };
                cb(is_on);
            }
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
            let ivars = self.ivars();
            if let Some(cb) = ivars.callback.borrow().as_ref() {
                let value: f32 = unsafe { msg_send![sender, value] };
                cb(value);
            }
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
            let ivars = self.ivars();
            if let Some(cb) = ivars.callback.borrow().as_ref() {
                let text: Option<Retained<NSString>> = unsafe { msg_send_id![sender, text] };
                let s = text.map(|ns| ns.to_string()).unwrap_or_default();
                cb(s);
            }
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

declare_class!(
    pub(crate) struct MetalView;

    unsafe impl ClassType for MetalView {
        type Super = UIView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMetalView";
    }

    impl DeclaredClass for MetalView {
        type Ivars = ();
    }

    unsafe impl MetalView {
        /// Override +layerClass to return [CAMetalLayer class].
        #[method(layerClass)]
        fn layer_class() -> *const std::ffi::c_void {
            objc2::class!(CAMetalLayer) as *const _ as *const std::ffi::c_void
        }
    }
);

impl MetalView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(());
        unsafe { msg_send_id![super(this), init] }
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
                if lw != 0.0 || lh != 0.0 {
                    crate::imp::schedule_layout_pass();
                }
            }
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
// OverlayPassthroughView — UIView subclass that only consumes touches
// in its subviews' frames.
//
// Used as the container for overlays with `BackdropMode::None`
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
