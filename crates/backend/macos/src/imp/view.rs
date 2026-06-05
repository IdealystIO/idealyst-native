//! [`FlippedView`] — `NSView` subclass that overrides `isFlipped`
//! to return `true`, giving us a top-left coordinate origin, AND
//! translates AppKit mouse events into the framework's `on_touch`
//! [`TouchHandler`] — the macOS analogue of iOS's `IdealystTouchView`.
//!
//! AppKit defaults to bottom-left origin (Y-up); Taffy emits frames
//! in top-left (Y-down). Resolving the mismatch at the view level
//! (one method override) instead of per-frame-write keeps the rest
//! of the backend identical to iOS / web / Android, all of which
//! already think in top-left.
//!
//! Every `create_view` / `create_pressable` / `create_link` returns
//! a `FlippedView`. AppKit-supplied leaves (NSTextField, NSButton,
//! NSSlider, NSSwitch, NSImageView) keep their default orientation
//! since their internal layout is opaque to us — the Taffy layout
//! pass sets their `frame` directly, computed in a flipped parent's
//! coordinate space.
//!
//! ## Mouse → `on_touch`
//!
//! `mouseDown:`/`mouseDragged:`/`mouseUp:` map to `TouchPhase::Began`/
//! `Moved`/`Ended` for the single mouse pointer. A handler is installed by
//! `Backend::install_touch_handler`; views without one let the event bubble up
//! the responder chain (so a click on a child without a handler still reaches a
//! parent that has one — exactly like iOS's `touchesBegan` super-call). macOS
//! points equal dp (no density scaling, unlike Android), and `isFlipped`
//! already makes `convertPoint:fromView:nil` return top-left coords matching
//! Taffy frames / the canvas Scene.

use std::cell::{Cell, RefCell};

use objc2::rc::Retained;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::{NSEvent, NSTextField, NSTextFieldCell, NSView};
use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSString};
use std::cell::Cell as StdCell;

use runtime_core::{TouchEvent, TouchHandler, TouchId, TouchPhase, TouchPoint};

/// Stable id for the single mouse pointer (macOS has no multitouch here).
const MOUSE_TOUCH_ID: u64 = 1;

pub struct FlippedViewIvars {
    /// Installed by `Backend::install_touch_handler`; `None` for the many
    /// views (containers, layout wrappers, native-control hosts) that carry
    /// no `on_touch`. `RefCell<Option<_>>` so it can be set after creation.
    handler: RefCell<Option<TouchHandler>>,
    /// True between a `mouseDown` we accepted (handler consumed/claimed) and
    /// the matching `mouseUp`, so drag/up events only dispatch for a gesture we
    /// actually started — mirrors iOS's `active_touches` gate.
    active: Cell<bool>,
}

declare_class!(
    pub struct FlippedView;

    unsafe impl ClassType for FlippedView {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystFlippedView";
    }

    impl DeclaredClass for FlippedView {
        type Ivars = FlippedViewIvars;
    }

    unsafe impl FlippedView {
        #[method(isFlipped)]
        fn is_flipped(&self) -> bool {
            true
        }

        // Register the FIRST click even when the window isn't key, so a tap on
        // the canvas/toolbar acts immediately (matches mobile tap behavior).
        #[method(acceptsFirstMouse:)]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[method(mouseDown:)]
        fn mouse_down(&self, event: &NSEvent) {
            if !self.dispatch_mouse(event, TouchPhase::Began) {
                let _: () = unsafe { msg_send![super(self), mouseDown: event] };
            }
        }

        #[method(mouseDragged:)]
        fn mouse_dragged(&self, event: &NSEvent) {
            if !self.dispatch_mouse(event, TouchPhase::Moved) {
                let _: () = unsafe { msg_send![super(self), mouseDragged: event] };
            }
        }

        #[method(mouseUp:)]
        fn mouse_up(&self, event: &NSEvent) {
            if !self.dispatch_mouse(event, TouchPhase::Ended) {
                let _: () = unsafe { msg_send![super(self), mouseUp: event] };
            }
        }
    }
);

impl FlippedView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(FlippedViewIvars {
            handler: RefCell::new(None),
            active: Cell::new(false),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Install (or replace) the `on_touch` handler. Called by
    /// `Backend::install_touch_handler`.
    pub(crate) fn set_handler(&self, handler: TouchHandler) {
        *self.ivars().handler.borrow_mut() = Some(handler);
    }

    /// `true` if an `on_touch` handler has been installed — i.e. this view is
    /// an interactive control. The private-layer passthrough hit-test uses
    /// this to decide whether a click should be CAPTURED (a real control) or
    /// fall through to the app window beneath (a bare layout container). The
    /// macOS analogue of iOS's `IdealystTouchView::has_handler`.
    pub(crate) fn has_handler(&self) -> bool {
        self.ivars().handler.borrow().is_some()
    }

    /// Translate one AppKit mouse event into a `TouchEvent` and dispatch it to
    /// the installed handler. Returns `true` when the event was handled (so the
    /// caller does NOT bubble it to `super`); `false` when there's no handler
    /// or it's a drag/up we didn't start, letting the responder chain carry it
    /// to an ancestor that does have a handler.
    fn dispatch_mouse(&self, event: &NSEvent, phase: TouchPhase) -> bool {
        // Snapshot the handler so we don't hold the ivar borrow across the
        // closure (it may re-enter the backend / signals).
        let handler = match self.ivars().handler.borrow().as_ref() {
            Some(h) => h.clone(),
            None => return false,
        };
        // Drag/up only matter if we accepted the down.
        if matches!(phase, TouchPhase::Moved | TouchPhase::Ended) && !self.ivars().active.get() {
            return false;
        }

        // Window coords (AppKit bottom-left) → this view's local coords. Since
        // `FlippedView` is `isFlipped`, the result is top-left (dp), matching
        // Taffy frames and the canvas Scene — no density scaling on macOS.
        let win: CGPoint = unsafe { msg_send![event, locationInWindow] };
        let local: CGPoint =
            unsafe { msg_send![self, convertPoint: win, fromView: std::ptr::null::<NSView>()] };

        // True top-left WINDOW coords. We can't reuse `local` (it's relative to
        // THIS view) for `window_position`: a drag handler that moves its own
        // view by the pointer delta would feed the moving frame back into the
        // delta → the widget never tracks the cursor and flickers (the macOS
        // "camera repositioning is janky" bug). Convert the window-space
        // location into the window's `contentView` — that view is the flipped,
        // full-window host, so its coordinate space IS top-left window space.
        let win_tl: CGPoint = unsafe {
            let window: *mut objc2::runtime::AnyObject = msg_send![self, window];
            if window.is_null() {
                local
            } else {
                let content: *mut objc2::runtime::AnyObject =
                    msg_send![window, contentView];
                if content.is_null() {
                    local
                } else {
                    msg_send![content, convertPoint: win, fromView: std::ptr::null::<NSView>()]
                }
            }
        };
        let ts: f64 = unsafe { msg_send![event, timestamp] };

        let ev = TouchEvent {
            id: TouchId(MOUSE_TOUCH_ID),
            phase,
            position: TouchPoint::new(local.x as f32, local.y as f32),
            window_position: TouchPoint::new(win_tl.x as f32, win_tl.y as f32),
            timestamp_ns: (ts * 1_000_000_000.0) as u64,
            force: None,
        };
        let response = (handler)(&ev);

        match phase {
            TouchPhase::Began => {
                if response.consumed || response.claim {
                    self.ivars().active.set(true);
                }
            }
            TouchPhase::Ended | TouchPhase::Cancelled => self.ivars().active.set(false),
            _ => {}
        }

        // Handled (don't bubble) if the handler consumed, or we're mid-gesture
        // it already accepted. An explicit IGNORED on `Began` returns false so
        // the event still bubbles to a parent handler.
        response.consumed || self.ivars().active.get()
    }
}

declare_class!(
    /// Non-interactive display label — the framework's `text()` primitive.
    ///
    /// Overrides `hitTest:` to return `nil` so mouse events (clicks AND
    /// scroll-wheel) pass THROUGH the text to whatever is behind/under it: a
    /// `Link` / `Pressable` parent that owns the tap, or an enclosing
    /// `NSScrollView` that owns the scroll. A plain `NSTextField` label sits in
    /// the hit-test path and `NSControl`'s mouse tracking SWALLOWS the click
    /// (and the scroll wheel) on a non-editable/non-selectable cell — so a
    /// button whose face is mostly its text never fired, and a page whose body
    /// is mostly text wouldn't scroll when the pointer was over the text. This
    /// is the macOS analogue of iOS labels' `userInteractionEnabled = false`.
    pub struct IdealystLabel;

    unsafe impl ClassType for IdealystLabel {
        type Super = NSTextField;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystLabel";
    }

    impl DeclaredClass for IdealystLabel {
        type Ivars = ();
    }

    unsafe impl IdealystLabel {
        #[method_id(hitTest:)]
        fn hit_test(&self, _point: CGPoint) -> Option<Retained<NSView>> {
            // Always decline: a display label must not capture mouse events.
            // AppKit then resolves the interactive view behind it.
            None
        }
    }
);

impl IdealystLabel {
    /// Create a hit-transparent display label configured like
    /// `+[NSTextField labelWithString:]` (non-editable, non-selectable, no
    /// bezel/border, transparent background). `create_text` applies the cell's
    /// wrap config + font/color styling on top, exactly as it did for the
    /// stock label.
    pub(crate) fn label_with_string(mtm: MainThreadMarker, s: &NSString) -> Retained<NSTextField> {
        let this: Retained<Self> =
            unsafe { msg_send_id![mtm.alloc::<Self>(), initWithFrame: CGRect::default()] };
        // Swap in an `IdealystLabelCell` so author `padding_*` on a `text()`
        // node insets the drawn glyphs (see that type). Must happen BEFORE the
        // configuration below so `setStringValue:` etc. land on the new cell.
        let cell = IdealystLabelCell::new(mtm);
        unsafe {
            let _: () = msg_send![&this, setCell: &*cell];
            let _: () = msg_send![&this, setStringValue: s];
            let _: () = msg_send![&this, setEditable: false];
            let _: () = msg_send![&this, setSelectable: false];
            let _: () = msg_send![&this, setBordered: false];
            let _: () = msg_send![&this, setBezeled: false];
            let _: () = msg_send![&this, setDrawsBackground: false];
        }
        // IdealystLabel IS-A NSTextField; expose it as the super type the rest
        // of the backend (measure_fn, MacosNode::Label) already speaks.
        Retained::into_super(this)
    }
}

/// Per-side text inset in points (top, left, bottom, right).
#[derive(Clone, Copy, Default)]
pub(crate) struct LabelInsets {
    pub top: f64,
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
}

pub(crate) struct CellIvars {
    insets: StdCell<LabelInsets>,
}

declare_class!(
    /// `NSTextFieldCell` subclass that draws its text inset by author
    /// `padding_*`.
    ///
    /// The framework's `StyleRules.padding_*` is applied by Taffy as the text
    /// node's padding rect, which grows the label's outer frame but does NOT
    /// push the glyphs in — `NSTextFieldCell` paints at `cellFrame.origin`, so
    /// `text(style = padding: 12)` rendered its glyphs flush in a corner with the
    /// padding space dumped on the opposite sides. Overriding
    /// `drawInteriorWithFrame:inView:` to inset the frame by the same padding
    /// puts the glyphs in the content rect — the macOS analogue of iOS's
    /// `IdealystLabel.drawText(in:)` inset.
    ///
    /// We intentionally do NOT touch `cellSizeForBounds:`: Taffy keeps the
    /// padding on the node (reserving the outer size) and hands the measure the
    /// content-box width, so the glyphs wrap to the same width they're drawn in.
    /// Inset only the drawing — sizing already accounts for the padding.
    pub(crate) struct IdealystLabelCell;

    unsafe impl ClassType for IdealystLabelCell {
        type Super = NSTextFieldCell;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystLabelCell";
    }

    impl DeclaredClass for IdealystLabelCell {
        type Ivars = CellIvars;
    }

    unsafe impl IdealystLabelCell {
        #[method(drawInteriorWithFrame:inView:)]
        fn draw_interior(&self, frame: CGRect, view: &NSView) {
            let i = self.ivars().insets.get();
            let inset = CGRect {
                origin: CGPoint {
                    x: frame.origin.x + i.left,
                    y: frame.origin.y + i.top,
                },
                size: CGSize {
                    width: (frame.size.width - i.left - i.right).max(0.0),
                    height: (frame.size.height - i.top - i.bottom).max(0.0),
                },
            };
            let _: () = unsafe { msg_send![super(self), drawInteriorWithFrame: inset, inView: view] };
        }
    }
);

impl IdealystLabelCell {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(CellIvars {
            insets: StdCell::new(LabelInsets::default()),
        });
        let empty = NSString::from_str("");
        unsafe { msg_send_id![super(this), initTextCell: &*empty] }
    }

    /// Update the per-side text insets. Called by `apply_style` from the text
    /// node's `padding_*`. Repaints via the control view so the change shows
    /// without a relayout (`setNeedsDisplay:` is an `NSView` method — an
    /// `NSCell` redraws through its `controlView`).
    pub(crate) fn set_insets(&self, insets: LabelInsets) {
        self.ivars().insets.set(insets);
        let cv: *mut NSView = unsafe { msg_send![self, controlView] };
        if !cv.is_null() {
            let _: () = unsafe { msg_send![cv, setNeedsDisplay: true] };
        }
    }
}

/// Set per-side text insets on `label` if its cell is an [`IdealystLabelCell`].
/// `apply_style` calls this for every `MacosNode::Label` so author `padding_*`
/// on a `text()` node insets the glyphs. No-op for any other cell class.
pub(crate) fn set_label_insets(label: &NSView, insets: LabelInsets) {
    let cell: *mut NSTextFieldCell = unsafe { msg_send![label, cell] };
    if cell.is_null() {
        return;
    }
    let cls = IdealystLabelCell::class();
    let is_ours: bool = unsafe { msg_send![cell, isKindOfClass: cls] };
    if !is_ours {
        return;
    }
    // SAFETY: just confirmed the dynamic class is `IdealystLabelCell`.
    let cell: &IdealystLabelCell = unsafe { &*(cell as *const IdealystLabelCell) };
    cell.set_insets(insets);
}

