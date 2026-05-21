use framework_core::primitives::key::{KeyDownHandler, KeyEvent, KeyOutcome};
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, MainThreadMarker, NSObject, NSRange, NSString};
use objc2_ui_kit::{UITextField, UITextView, UIView};
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
            framework_core::set_safe_area_insets(framework_core::EdgeInsets {
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
    /// vocabulary documented on `framework_core::primitives::key`:
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
