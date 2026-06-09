//! NSObject subclasses that bridge AppKit `target/action` selectors
//! to Rust closures. Mirrors the iOS backend's
//! `callbacks.rs` shape — different subclass per parameter shape so
//! each method's signature matches what the AppKit sender expects.
//!
//! Each control's `setTarget:` / `setAction:` keeps the target as a
//! **weak** reference. The owning [`MacosBackend`] stashes the
//! `Retained<...>` in `callback_targets` for the backend's lifetime
//! so the target survives long enough for every dispatched action
//! to find a live receiver.

use objc2::rc::Retained;
use objc2::runtime::{NSObject as NSObjectRuntime, NSObjectProtocol};
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::NSControl;
use objc2_foundation::{MainThreadMarker, NSObject};
use std::cell::RefCell;
use std::rc::Rc;

// =========================================================================
// CallbackTarget — fires a `Fn()` from `-(IBAction)invoke:(id)sender`.
// Used for NSButton clicks.
// =========================================================================

pub(crate) struct CallbackTargetIvars {
    pub(crate) callback: RefCell<Option<Rc<dyn Fn()>>>,
}

declare_class!(
    pub(crate) struct CallbackTarget;

    unsafe impl ClassType for CallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystCallbackTarget";
    }

    impl DeclaredClass for CallbackTarget {
        type Ivars = CallbackTargetIvars;
    }

    unsafe impl NSObjectProtocol for CallbackTarget {}

    unsafe impl CallbackTarget {
        #[method(invoke:)]
        fn invoke(&self, _sender: &NSObjectRuntime) {
            if let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() {
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
// BoolCallbackTarget — fires a `Fn(bool)` from an NSSwitch / NSButton
// in checkbox mode. Reads the sender's `state` (NSControlStateValueOn
// = 1, NSControlStateValueOff = 0) and forwards.
// =========================================================================

pub(crate) struct BoolCallbackTargetIvars {
    pub(crate) callback: RefCell<Option<Rc<dyn Fn(bool)>>>,
}

declare_class!(
    pub(crate) struct BoolCallbackTarget;

    unsafe impl ClassType for BoolCallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystBoolCallbackTarget";
    }

    impl DeclaredClass for BoolCallbackTarget {
        type Ivars = BoolCallbackTargetIvars;
    }

    unsafe impl NSObjectProtocol for BoolCallbackTarget {}

    unsafe impl BoolCallbackTarget {
        #[method(invoke:)]
        fn invoke(&self, sender: &NSObjectRuntime) {
            if let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() {
                // `state` is an NSInteger; 1 = on, 0 = off. NSSwitch
                // uses 1/0 directly; NSButton in checkbox/toggle style
                // uses NSControlStateValueOn/Off which are also 1/0.
                let state: isize = unsafe { msg_send![sender, state] };
                cb(state == 1);
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
// FloatCallbackTarget — fires a `Fn(f32)` from an NSSlider / NSControl
// whose `value` is a `double`. Reads `doubleValue` and forwards as f32.
// =========================================================================

pub(crate) struct FloatCallbackTargetIvars {
    pub(crate) callback: RefCell<Option<Rc<dyn Fn(f32)>>>,
}

declare_class!(
    pub(crate) struct FloatCallbackTarget;

    unsafe impl ClassType for FloatCallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystFloatCallbackTarget";
    }

    impl DeclaredClass for FloatCallbackTarget {
        type Ivars = FloatCallbackTargetIvars;
    }

    unsafe impl NSObjectProtocol for FloatCallbackTarget {}

    unsafe impl FloatCallbackTarget {
        #[method(invoke:)]
        fn invoke(&self, sender: &NSObjectRuntime) {
            if let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() {
                let value: f64 = unsafe { msg_send![sender, doubleValue] };
                cb(value as f32);
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
// StringCallbackTarget — fires a `Fn(String)` from an NSTextField's
// `controlTextDidChange:` notification (NotificationCenter-driven,
// not target/action).
//
// The receiver method is `controlTextDidChange:` — NSTextField fires
// this on every keystroke when its delegate or notification observer
// implements it. We register the same selector against
// NSNotificationCenter via `addObserver:selector:name:object:`.
// =========================================================================

pub(crate) struct StringCallbackTargetIvars {
    pub(crate) callback: RefCell<Option<Rc<dyn Fn(String)>>>,
}

declare_class!(
    pub(crate) struct StringCallbackTarget;

    unsafe impl ClassType for StringCallbackTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystStringCallbackTarget";
    }

    impl DeclaredClass for StringCallbackTarget {
        type Ivars = StringCallbackTargetIvars;
    }

    unsafe impl NSObjectProtocol for StringCallbackTarget {}

    unsafe impl StringCallbackTarget {
        #[method(controlTextDidChange:)]
        fn control_text_did_change(&self, notification: &NSObjectRuntime) {
            if let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() {
                // `notification.object` is the NSTextField; ask it for
                // its `stringValue`. NSTextField is an NSControl.
                let sender: *mut NSControl = unsafe { msg_send![notification, object] };
                if sender.is_null() {
                    return;
                }
                let ns: *mut objc2_foundation::NSString =
                    unsafe { msg_send![sender, stringValue] };
                if ns.is_null() {
                    return;
                }
                let ns_ref: &objc2_foundation::NSString = unsafe { &*ns };
                cb(ns_ref.to_string());
            }
        }

        /// `NSTextDidChangeNotification` — fires for `NSTextView`
        /// (multi-line editor backing our `TextArea` primitive)
        /// on every edit. NSTextView is NOT an NSControl, so we
        /// can't reach its content through `stringValue`; the
        /// canonical accessor is `string` (returns `NSString*`).
        /// Same callback Rc carries the value back to the
        /// framework.
        #[method(textDidChange:)]
        fn text_did_change(&self, notification: &NSObjectRuntime) {
            if let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() {
                let sender: *mut NSObjectRuntime =
                    unsafe { msg_send![notification, object] };
                if sender.is_null() {
                    return;
                }
                let ns: *mut objc2_foundation::NSString =
                    unsafe { msg_send![sender, string] };
                if ns.is_null() {
                    return;
                }
                let ns_ref: &objc2_foundation::NSString = unsafe { &*ns };
                cb(ns_ref.to_string());
            }
        }
    }
);

impl StringCallbackTarget {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        callback: Rc<dyn Fn(String)>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(StringCallbackTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// ScrollObserverTarget — bridges `NSViewBoundsDidChangeNotification`
// (fired on an NSScrollView's clipView whenever the user scrolls) to
// a Rust `Fn(f32, f32)` closure.
//
// NSScrollView doesn't expose a delegate \u{2014} the canonical macOS
// pattern is to flip the contentView's `postsBoundsChangedNotifications`
// on, then observe `NSView.boundsDidChangeNotification` keyed on that
// clipView. The observer reads the clipView's `documentVisibleRect`
// origin to recover the current scroll offset.
// =========================================================================

pub(crate) struct ScrollObserverTargetIvars {
    pub(crate) callback: RefCell<Option<Rc<dyn Fn(f32, f32)>>>,
}

declare_class!(
    pub(crate) struct ScrollObserverTarget;

    unsafe impl ClassType for ScrollObserverTarget {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystScrollObserverTarget";
    }

    impl DeclaredClass for ScrollObserverTarget {
        type Ivars = ScrollObserverTargetIvars;
    }

    unsafe impl NSObjectProtocol for ScrollObserverTarget {}

    unsafe impl ScrollObserverTarget {
        #[method(boundsDidChange:)]
        fn bounds_did_change(&self, notification: &NSObjectRuntime) {
            let Some(cb) = self.ivars().callback.borrow().as_ref().cloned() else {
                return;
            };
            // `notification.object` is the NSClipView whose bounds
            // changed. Its `bounds.origin` is the current scroll
            // offset in the clip view's coordinate space \u{2014}
            // identical units to web/iOS (CSS pixels / points).
            let clip: *mut objc2::runtime::AnyObject =
                unsafe { msg_send![notification, object] };
            if clip.is_null() {
                return;
            }
            let bounds: objc2_foundation::CGRect = unsafe { msg_send![clip, bounds] };
            cb(bounds.origin.x as f32, bounds.origin.y as f32);
        }
    }
);

impl ScrollObserverTarget {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        callback: Rc<dyn Fn(f32, f32)>,
    ) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ScrollObserverTargetIvars {
            callback: RefCell::new(Some(callback)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}

// =========================================================================
// PrivateLayerPassthroughView — the screen_recorder `PrivateLayer` overlay
// window's root content view. The macOS analogue of iOS's
// `PrivateLayerPassthroughView` + `PassthroughWindow` (one view does both
// jobs here because AppKit routes passthrough purely through `hitTest:`).
//
// The overlay lives in its own borderless `NSWindow` above the app window.
// Its content is a viewport-spanning, TRANSPARENT flex root (a Taffy root
// sized to the window) with the actual controls — a toolbar, a recording
// preview — nested deep inside and sparse. AppKit's default
// `NSView.hitTest:` returns the deepest subview whose `frame` contains the
// point; for a full-screen transparent content view that's the content view
// itself everywhere, so EVERY click would be delivered to this overlay window
// and the app's canvas beneath would never see it ("can't draw at all").
//
// So we override `hitTest:`: build a HitNode tree from the live subview
// subtree and ask the host-tested `region_blocks_click` whether the point
// lands on a view that actually wants it — an interactive control (a
// `FlippedView` with an installed `on_touch` handler) or a view that paints
// visible content (a non-clear layer background). If it does, defer to
// `super` so AppKit resolves the precise deep subview. If it doesn't, return
// `nil` so AppKit proceeds to the window beneath and the click reaches the
// app's drawable canvas. This is the exact "captures iff control-or-visible,
// recurse through transparent containers" behavior the iOS module implements,
// diverging only in the AppKit mechanism (`hitTest:`-returns-nil vs UIKit's
// `pointInside:` + window override).
// =========================================================================

use objc2_app_kit::NSView;
use objc2_foundation::{CGPoint, CGRect, CGSize, NSArray};

extern "C" {
    /// CoreGraphics: alpha component of a `CGColorRef` (0.0 = fully
    /// transparent). Linked from the system CoreGraphics framework.
    fn CGColorGetAlpha(color: *const std::ffi::c_void) -> objc2_foundation::CGFloat;
}

/// Build the [`HitNode`] tree for `view`'s subviews (recursively): each
/// subview's `frame` in the parent's coordinate space + whether it itself
/// captures clicks. Hidden subviews are skipped (they can't be hit).
fn private_layer_hit_nodes(
    view: &NSView,
) -> Vec<crate::private_layer_hittest::HitNode> {
    let subviews: Retained<NSArray<NSView>> = unsafe { msg_send_id![view, subviews] };
    let mut nodes = Vec::new();
    for sub in subviews.iter() {
        let hidden: bool = unsafe { msg_send![&*sub, isHidden] };
        if hidden {
            continue;
        }
        let frame: CGRect = unsafe { msg_send![&*sub, frame] };
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

/// A single view captures clicks if it's an interactive control (a
/// `FlippedView` with an installed `on_touch` handler) or paints visible
/// content (a non-clear layer background, alpha > 0). Mirrors the iOS
/// `private_layer_view_captures`.
fn private_layer_view_captures(view: &NSView) -> bool {
    // Interactive control: a FlippedView with a handler installed. The Taffy
    // frames place controls with plain frames (no transform), so the recursion
    // in `region_blocks_click` lines up with these frames exactly.
    let flipped_cls = super::view::FlippedView::class();
    let is_flipped: bool = unsafe { msg_send![view, isKindOfClass: flipped_cls] };
    if is_flipped {
        // SAFETY: dynamic class just confirmed `FlippedView`; the cast reads
        // its ivar-backed `has_handler` accessor.
        let fv: &super::view::FlippedView =
            unsafe { &*(view as *const NSView as *const super::view::FlippedView) };
        if fv.has_handler() {
            return true;
        }
    }
    // Visible content: a layer-backed view with a non-clear backgroundColor
    // (alpha > 0). A plain transparent layout container has either no layer or
    // a nil/clear `backgroundColor` → falls through. `setWantsLayer:` is set by
    // `apply_style_to_view` whenever a background is applied, so a styled panel
    // has a layer here.
    let layer: *mut NSObject = unsafe { msg_send![view, layer] };
    if !layer.is_null() {
        // CRITICAL: `-[CALayer backgroundColor]` returns a typed `CGColorRef`
        // (objc2 encoding `^{CGColor=}`), NOT a plain `void*` (`^v`). Receiving
        // it as `*const c_void` trips objc2's msg_send encoding check and
        // SIGABRTs inside this `hitTest:` (a non-unwinding callback) — the
        // crash on the first click. Use the backend's encoding-correct
        // `CGColorRef` newtype, then read its inner pointer for the C alpha fn.
        let cg: super::CGColorRef = unsafe { msg_send![layer, backgroundColor] };
        if !cg.0.is_null() && unsafe { CGColorGetAlpha(cg.0) } > 0.0 {
            return true;
        }
    }
    false
}

/// Recursive hit-test decision for [`PrivateLayerPassthroughView`]. Returns
/// `true` if `point` (in the content view's local, top-left coordinate space)
/// lands on a subview that should CAPTURE the click. Builds the live HitNode
/// tree and delegates the recursion + coordinate conversion to the
/// host-tested [`region_blocks_click`](crate::private_layer_hittest::region_blocks_click).
pub(crate) fn private_layer_blocks_click(view: &NSView, point: CGPoint) -> bool {
    let nodes = private_layer_hit_nodes(view);
    crate::private_layer_hittest::region_blocks_click(&nodes, point.x, point.y)
}

declare_class!(
    pub(crate) struct PrivateLayerPassthroughView;

    unsafe impl ClassType for PrivateLayerPassthroughView {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacosPrivateLayerPassthroughView";
    }

    impl DeclaredClass for PrivateLayerPassthroughView {
        type Ivars = ();
    }

    unsafe impl PrivateLayerPassthroughView {
        // Flipped origin so the content view's local coordinate space is
        // top-left (Y-down), matching Taffy frames + the iOS-authored
        // `region_blocks_click` math. Without this the subview frames (Taffy,
        // top-left) and the hit-test point (AppKit, bottom-left) would be in
        // mismatched spaces and the toolbar would be hit at the wrong Y.
        #[method(isFlipped)]
        fn is_flipped(&self) -> bool {
            true
        }

        #[method_id(hitTest:)]
        fn hit_test(&self, point: CGPoint) -> Option<Retained<NSView>> {
            // `point` arrives in the SUPERVIEW's coordinate space (AppKit's
            // `hitTest:` contract). Convert it into THIS view's local space —
            // `convertPoint:fromView:` against our `superview` — before running
            // the subtree recursion, whose HitNode frames are all in our local
            // (Taffy, top-left, since `isFlipped`) coordinate space. When there
            // is no superview yet (mid-attach) the point is already local.
            let this: &NSView = unsafe { &*(self as *const Self as *const NSView) };
            let superview: *mut NSView = unsafe { msg_send![this, superview] };
            let local: CGPoint = if superview.is_null() {
                point
            } else {
                unsafe { msg_send![this, convertPoint: point, fromView: superview] }
            };
            if private_layer_blocks_click(this, local) {
                // A control IS under the click → let `super` resolve the
                // precise deep subview so the click reaches the actual button.
                unsafe { msg_send_id![super(self), hitTest: point] }
            } else {
                // No private-layer control under the click → decline so AppKit
                // delivers the event to the window beneath (the app's canvas).
                None
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
// LayoutObserverView — re-runs the Taffy layout pass on window resize.
//
// `Backend::finish` lays the tree out ONCE, against the host content view's
// bounds at mount. Reactive Effects schedule their own passes via
// `apply_style` → `schedule_layout_pass`, but a raw window resize (drag the
// frame, full-screen toggle, titlebar/toolbar show-hide) produces NO reactive
// change — the framework never re-enters `finish`, so the root view and every
// child keep their stale mount-time frames and the layout no longer fills the
// window.
//
// This is the AppKit analogue of the iOS backend's `LayoutObserverView`
// (`backend_ios_mobile::imp::callbacks`). The mechanism differs — UIKit calls
// `layoutSubviews` on every bounds change; AppKit has no such hook, so we
// override `setFrameSize:`, which AppKit invokes when it autoresizes this view
// in response to the host content view changing size. The observable behavior
// converges: a real size change mirrors the reactive `viewport_size()` signal
// and kicks a coalesced `schedule_layout_pass`, exactly like iOS.
//
// The observer is an invisible (hidden → excluded from drawing + hit-testing),
// zero-cost subview of the host root pinned to fill it via the
// flexible-width|flexible-height autoresizing mask. It carries no app content;
// its only job is to receive `setFrameSize:`.
//
// We dedupe by remembering the last size we re-laid out at, so the redundant
// `setFrameSize:` AppKit emits for an unchanged size (and the initial
// `setFrame:` in `set_host_root`, which is pre-seeded to match) produce no
// extra passes.
// =========================================================================

pub(crate) struct LayoutObserverIvars {
    last_size: std::cell::Cell<(f32, f32)>,
}

declare_class!(
    pub(crate) struct LayoutObserverView;

    unsafe impl ClassType for LayoutObserverView {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystMacosLayoutObserverView";
    }

    impl DeclaredClass for LayoutObserverView {
        type Ivars = LayoutObserverIvars;
    }

    unsafe impl LayoutObserverView {
        #[method(setFrameSize:)]
        fn set_frame_size(&self, size: CGSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            let next = (size.width as f32, size.height as f32);
            let last = self.ivars().last_size.get();
            // The react/skip/dedupe decision is the host-tested pure function
            // `resize_observer_reaction` (see `layout_policy`).
            let reaction = crate::layout_policy::resize_observer_reaction(last, next);
            if reaction.mirror_viewport {
                self.ivars().last_size.set(next);
                // Push to the reactive viewport signal so `viewport_size()`
                // subscribers (responsive containers, breakpoint hooks, theme-
                // cohort restyle) re-fire. Safe to call synchronously here: a
                // window-resize `setFrameSize:` runs on the main runloop OUTSIDE
                // any framework borrow window, unlike `finish` (which defers
                // this mirror precisely because it runs mid-borrow).
                runtime_core::set_viewport_size(runtime_core::ViewportSize {
                    width: next.0,
                    height: next.1,
                });
            }
            if reaction.schedule_pass {
                // Run the pass SYNCHRONOUSLY, in this same `setFrameSize:` turn,
                // rather than only deferring it. The host view has ALREADY resized;
                // a deferred-only pass re-centers the stage a frame later, so the
                // canvas "sticks to the top, then jumps down" on resize. `schedule`
                // queues it (and arms the deferred fallback); `flush` runs it now
                // against the just-updated viewport + restyled frame, so the resize
                // and the re-centering land together. No-op if nothing's queued.
                crate::imp::schedule_layout_pass();
                crate::imp::flush_pending_layout_pass();
            }
        }
    }
);

impl LayoutObserverView {
    /// Construct the observer with `last_size` pre-seeded to the host's
    /// current bounds, so the initial `setFrame:` in `set_host_root` (which
    /// matches that size) is a no-op and doesn't fire a redundant viewport
    /// mirror / layout pass while the backend is still borrowed.
    pub(crate) fn new(mtm: MainThreadMarker, seed: CGSize) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(LayoutObserverIvars {
            last_size: std::cell::Cell::new((seed.width as f32, seed.height as f32)),
        });
        unsafe { msg_send_id![super(this), init] }
    }
}
