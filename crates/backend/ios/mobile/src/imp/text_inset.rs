//! `IdealystLabel` — a `UILabel` subclass that honors per-side
//! text insets.
//!
//! The framework's `StyleRules.padding_*` fields are applied by
//! Taffy as the node's padding rect, which insets a container's
//! children inside its bounds. UILabel has no children — its drawn
//! glyphs are intrinsic to the view — so Taffy padding on a `text(...)`
//! node grows the label's outer frame but does nothing to push the
//! glyphs in: by default `UILabel.drawText(in:)` paints at
//! `bounds.origin`, ignoring any margins.
//!
//! Result before this subclass: a `text(style = NavLink)` with
//! `padding_horizontal: 12` rendered text flush against the parent's
//! content edge — the 12 pt was added to the label's outer width but
//! the glyphs sat at x = 0.
//!
//! `IdealystLabel` exposes a `text_insets: UIEdgeInsets` ivar. The
//! iOS backend's `apply_style` writes Taffy padding values into it,
//! and the overridden `drawText(in:)` / `intrinsicContentSize` /
//! `sizeThatFits:` honor the insets so the glyphs land in the
//! content area and the measure_fn reports a size that includes the
//! padded chrome.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{UIColor, UILabel, UITextField, UITextView, UIView};

/// Per-side text inset, in points. Layout matches `UIEdgeInsets`
/// (top, left, bottom, right) so the same struct can be sent over
/// the obj-c boundary via `setTextInsets:` and stored on the label
/// without per-field marshalling.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct TextInsets {
    pub top: CGFloat,
    pub left: CGFloat,
    pub bottom: CGFloat,
    pub right: CGFloat,
}

unsafe impl Encode for TextInsets {
    const ENCODING: Encoding = Encoding::Struct(
        "UIEdgeInsets",
        &[CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING, CGFloat::ENCODING],
    );
}

pub(crate) struct LabelIvars {
    insets: Cell<TextInsets>,
}

declare_class!(
    pub(crate) struct IdealystLabel;

    unsafe impl ClassType for IdealystLabel {
        type Super = UILabel;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystLabel";
    }

    impl DeclaredClass for IdealystLabel {
        type Ivars = LabelIvars;
    }

    unsafe impl IdealystLabel {
        /// `drawText(in:)` is the entry point UILabel uses to paint
        /// glyphs into a rect inside its bounds. Insetting the rect
        /// before forwarding to super is the canonical way to add
        /// padding to a UILabel — no need to override `drawRect:` or
        /// touch the text-layout pipeline.
        #[method(drawTextInRect:)]
        fn draw_text_in_rect(&self, rect: CGRect) {
            let insets = self.ivars().insets.get();
            let inset_rect = CGRect {
                origin: objc2_foundation::CGPoint {
                    x: rect.origin.x + insets.left,
                    y: rect.origin.y + insets.top,
                },
                size: CGSize {
                    width: (rect.size.width - insets.left - insets.right).max(0.0),
                    height: (rect.size.height - insets.top - insets.bottom).max(0.0),
                },
            };
            let _: () = unsafe { msg_send![super(self), drawTextInRect: inset_rect] };
        }

        /// `sizeThatFits:` is what Taffy's `measure_fn` calls during
        /// layout to discover the label's natural size given a
        /// container width. Account for the insets so the resolver
        /// reserves room for the padding chrome alongside the
        /// glyphs — otherwise text would wrap as if the padding
        /// didn't exist, then get clipped when drawn into the inset
        /// rect.
        #[method(sizeThatFits:)]
        fn size_that_fits(&self, size: CGSize) -> CGSize {
            let insets = self.ivars().insets.get();
            let avail = CGSize {
                width: (size.width - insets.left - insets.right).max(0.0),
                height: (size.height - insets.top - insets.bottom).max(0.0),
            };
            let inner: CGSize = unsafe { msg_send![super(self), sizeThatFits: avail] };
            CGSize {
                width: inner.width + insets.left + insets.right,
                height: inner.height + insets.top + insets.bottom,
            }
        }

        /// `intrinsicContentSize` is the autolayout entry point for
        /// the same question `sizeThatFits:` answers — UIKit reads
        /// it when no width has been pinned. Adding the insets here
        /// keeps the label honest in both routes.
        #[method(intrinsicContentSize)]
        fn intrinsic_content_size(&self) -> CGSize {
            let insets = self.ivars().insets.get();
            let inner: CGSize = unsafe { msg_send![super(self), intrinsicContentSize] };
            CGSize {
                width: inner.width + insets.left + insets.right,
                height: inner.height + insets.top + insets.bottom,
            }
        }

        /// Obj-c-exposed setter for the inset ivar. Lets
        /// `backend-ios-core::style::apply_text_insets_if_label`
        /// write the values via a plain `msg_send![view,
        /// setTextInsets: insets]` without needing a Rust-level dep
        /// on this crate.
        #[method(setTextInsets:)]
        fn set_text_insets_objc(&self, insets: TextInsets) {
            self.ivars().insets.set(insets);
            let _: () = unsafe { msg_send![self, setNeedsDisplay] };
            let _: () = unsafe { msg_send![self, invalidateIntrinsicContentSize] };
        }
    }
);

impl IdealystLabel {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(LabelIvars {
            insets: Cell::new(TextInsets::default()),
        });
        unsafe { msg_send_id![super(this), init] }
    }

}

// =========================================================================
// IdealystTextField — a `UITextField` subclass that (1) insets its text by the
// author's `padding_*` and (2) drives `StateBits::FOCUSED` from first-responder
// changes, so the macOS/web focus ring + padded input render on iOS too.
//
// A plain UITextField paints its text flush to the border — author padding only
// grew the outer frame (same problem `IdealystLabel` solves for glyphs) — and
// offers no focus hook the framework's event-driven state path can use.
// Overriding the text-rect methods adds the inset; overriding
// become/resignFirstResponder fires focus. The iOS analogue of macOS's
// `VCenterTextFieldCell`. It reuses `TextInsets` + the `setTextInsets:`
// selector so the SAME `apply_text_insets_if_label` path feeds both classes.
// =========================================================================

pub(crate) struct TextFieldIvars {
    insets: Cell<TextInsets>,
    /// The framework's `FOCUSED` state driver (`StyleOps::attach_states`).
    focus_setter: RefCell<Option<Rc<dyn Fn(bool)>>>,
    /// The `text_input`'s focus notifier
    /// (`TextInputOps::set_text_input_focus_handler`): the author's
    /// `on_focus`, and the robot's `focused` — the field a window-level
    /// robot key lands in. A separate slot from `focus_setter`: a field
    /// can have either without the other.
    focus_handler: RefCell<Option<Rc<dyn Fn(bool)>>>,
    /// The placeholder text and its ink, kept together: UIKit's plain
    /// `placeholder` setter drops a colored `attributedPlaceholder`, so a
    /// text change must rebuild it with the ink (`apply_placeholder`).
    placeholder: RefCell<Option<String>>,
    placeholder_color: RefCell<Option<Retained<UIColor>>>,
}

declare_class!(
    pub(crate) struct IdealystTextField;

    unsafe impl ClassType for IdealystTextField {
        type Super = UITextField;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystTextField";
    }

    impl DeclaredClass for IdealystTextField {
        type Ivars = TextFieldIvars;
    }

    unsafe impl IdealystTextField {
        /// Rect for the resting text. Inset the default by the author padding.
        #[method(textRectForBounds:)]
        fn text_rect_for_bounds(&self, bounds: CGRect) -> CGRect {
            let base: CGRect = unsafe { msg_send![super(self), textRectForBounds: bounds] };
            self.inset_rect(base)
        }

        /// Rect while editing (caret + live text). Same inset so the text
        /// doesn't shift left when the field gains focus.
        #[method(editingRectForBounds:)]
        fn editing_rect_for_bounds(&self, bounds: CGRect) -> CGRect {
            let base: CGRect = unsafe { msg_send![super(self), editingRectForBounds: bounds] };
            self.inset_rect(base)
        }

        /// Rect for the placeholder — must match the text rect. We inset
        /// super's *text* rect, NOT super's placeholder rect: UITextField's
        /// default `placeholderRectForBounds:` internally calls
        /// `textRectForBounds:` (our override, already inset), so insetting the
        /// super placeholder rect would double the padding and shove the
        /// placeholder right. Deriving from super's textRect gives a single,
        /// matching inset.
        #[method(placeholderRectForBounds:)]
        fn placeholder_rect_for_bounds(&self, bounds: CGRect) -> CGRect {
            let base: CGRect = unsafe { msg_send![super(self), textRectForBounds: bounds] };
            self.inset_rect(base)
        }

        /// Attach hook of the attach-safe `focus()`: a field focused before
        /// it had a window becomes first responder once it gets one
        /// (`pending_focus`).
        #[method(didMoveToWindow)]
        fn did_move_to_window(&self) {
            let _: () = unsafe { msg_send![super(self), didMoveToWindow] };
            crate::imp::pending_focus::view_moved_to_window(self);
        }

        /// Focus gained → flip FOCUSED on (the focus ring).
        #[method(becomeFirstResponder)]
        fn become_first_responder(&self) -> bool {
            let became: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if became {
                self.fire_focus(true);
            }
            became
        }

        /// Focus lost → flip FOCUSED off. Only a field that HELD focus
        /// reports the loss: `blur()` resigns unconditionally, and an
        /// unfocused field's `on_focus(false)` would be a blur that never
        /// happened (the web fires no `blur` for one).
        #[method(resignFirstResponder)]
        fn resign_first_responder(&self) -> bool {
            let held: bool = unsafe { msg_send![self, isFirstResponder] };
            let resigned: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if held && resigned {
                self.fire_focus(false);
                // Focus left for nowhere with an overlay open: keep a
                // responder for its Escape (`keyboard::claim_key_focus`).
                crate::imp::keyboard::claim_key_focus_soon();
            }
            resigned
        }

        /// A hardware Escape while the field has focus closes the topmost
        /// overlay (`keyboard::escape_key_commands`).
        #[method_id(keyCommands)]
        fn key_commands(&self) -> Option<Retained<NSObject>> {
            crate::imp::keyboard::escape_key_commands(unsafe { msg_send_id![super(self), keyCommands] })
        }

        #[method(idealystEscapeDismiss:)]
        fn escape_dismiss(&self, _command: *mut NSObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystTextField::escapeDismiss", || {
                crate::imp::keyboard::escape_dismiss();
            });
        }

        /// Obj-c setter for the inset ivar — `apply_text_insets_if_label`
        /// sends `setTextInsets:` exactly as it does for `IdealystLabel`.
        #[method(setTextInsets:)]
        fn set_text_insets_objc(&self, insets: TextInsets) {
            self.ivars().insets.set(insets);
            let _: () = unsafe { msg_send![self, setNeedsLayout] };
        }
    }
);

impl IdealystTextField {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(TextFieldIvars {
            insets: Cell::new(TextInsets::default()),
            focus_setter: RefCell::new(None),
            focus_handler: RefCell::new(None),
            placeholder: RefCell::new(None),
            placeholder_color: RefCell::new(None),
        });
        unsafe { msg_send_id![super(this), init] }
    }

    /// Set the placeholder text (`None` clears), keeping its ink.
    pub(crate) fn set_placeholder_text(&self, text: Option<&str>) {
        *self.ivars().placeholder.borrow_mut() = text.map(str::to_owned);
        self.apply_placeholder();
    }

    /// Set the placeholder ink (`None`: UIKit's own), keeping its text.
    pub(crate) fn set_placeholder_color(&self, color: Option<Retained<UIColor>>) {
        *self.ivars().placeholder_color.borrow_mut() = color;
        self.apply_placeholder();
    }

    /// The placeholder as UIKit takes it: plain with no ink (UIKit's own
    /// gray), else attributed with only the foreground color set — the
    /// field's font still applies to the attributes it leaves out.
    fn apply_placeholder(&self) {
        let text = self.ivars().placeholder.borrow().clone();
        let color = self.ivars().placeholder_color.borrow().clone();
        // Clear the attributed one first: the plain setter keeps the ink of
        // an attributed placeholder already set, so `None` would not return
        // to UIKit's own.
        let _: () = unsafe { msg_send![self, setAttributedPlaceholder: std::ptr::null::<NSObject>()] };
        match (text, color) {
            (None, _) => unsafe { self.setPlaceholder(None) },
            (Some(t), None) => unsafe { self.setPlaceholder(Some(&NSString::from_str(&t))) },
            (Some(t), Some(c)) => unsafe {
                let attributed = colored_attributed_string(&t, &c);
                let _: () = msg_send![self, setAttributedPlaceholder: &*attributed];
            },
        }
    }

    /// Inset the text rect HORIZONTALLY only. UITextField already centers a
    /// single line vertically within the rect, so the author's vertical padding
    /// is realized by the field's taller frame (Taffy adds `padding_*` to the
    /// node), NOT by shrinking the text rect — insetting top+bottom here would
    /// collapse the line to nothing on a short field (the "no text" bug). Left/
    /// right insets ARE applied so the text doesn't touch the border.
    fn inset_rect(&self, r: CGRect) -> CGRect {
        let i = self.ivars().insets.get();
        CGRect {
            origin: CGPoint { x: r.origin.x + i.left, y: r.origin.y },
            size: CGSize {
                width: (r.size.width - i.left - i.right).max(0.0),
                height: r.size.height,
            },
        }
    }

    /// Install the focus-state setter (from `StyleOps::attach_states`).
    pub(crate) fn set_focus_setter(&self, setter: Rc<dyn Fn(bool)>) {
        *self.ivars().focus_setter.borrow_mut() = Some(setter);
    }

    /// Install the `text_input`'s focus notifier.
    pub(crate) fn set_focus_handler(&self, handler: Rc<dyn Fn(bool)>) {
        *self.ivars().focus_handler.borrow_mut() = Some(handler);
    }

    /// Both slots are cloned out before either runs: the author's
    /// `on_focus` may re-enter the field (focus another one, blur this).
    fn fire_focus(&self, on: bool) {
        let state = self.ivars().focus_setter.borrow().clone();
        let handler = self.ivars().focus_handler.borrow().clone();
        if let Some(cb) = state {
            cb(on);
        }
        if let Some(cb) = handler {
            cb(on);
        }
    }
}

/// `text` as an `NSAttributedString` whose only attribute is its
/// foreground color (`NSForegroundColorAttributeName`, the string
/// `"NSColor"`).
fn colored_attributed_string(text: &str, color: &UIColor) -> Retained<NSObject> {
    unsafe {
        let key = NSString::from_str("NSColor");
        let attrs: Retained<NSObject> =
            msg_send_id![objc2::class!(NSDictionary), dictionaryWithObject: color, forKey: &*key];
        let alloc: *mut NSObject = msg_send![objc2::class!(NSAttributedString), alloc];
        let ns = NSString::from_str(text);
        let inited: *mut NSObject = msg_send![alloc, initWithString: &*ns, attributes: &*attrs];
        Retained::from_raw(inited).expect("NSAttributedString init returned nil")
    }
}

/// `field` as the `IdealystTextField` every `text_input` is, if it is one.
pub(crate) fn as_idealyst_text_field(field: &UITextField) -> Option<&IdealystTextField> {
    let is_ours: bool = unsafe { msg_send![field, isKindOfClass: IdealystTextField::class()] };
    // SAFETY: just confirmed the dynamic class is `IdealystTextField`.
    is_ours.then(|| unsafe { &*(field as *const UITextField as *const IdealystTextField) })
}

/// Install a focus setter on `view` if it is an `IdealystTextField`; no-op
/// otherwise. The iOS analogue of macOS's `set_text_field_focus_setter`.
pub(crate) fn set_text_field_focus_setter(view: &UIView, setter: Rc<dyn Fn(bool)>) {
    let cls = IdealystTextField::class();
    let is_ours: bool = unsafe { msg_send![view, isKindOfClass: cls] };
    if !is_ours {
        return;
    }
    // SAFETY: just confirmed the dynamic class is `IdealystTextField`.
    let field: &IdealystTextField =
        unsafe { &*(view as *const UIView as *const IdealystTextField) };
    field.set_focus_setter(setter);
}

/// Install a `text_input`'s focus notifier on `view` if it is an
/// `IdealystTextField`; no-op otherwise. The iOS analogue of macOS's
/// `set_text_field_author_focus`.
pub(crate) fn set_text_field_focus_handler(view: &UIView, handler: Rc<dyn Fn(bool)>) {
    let is_ours: bool = unsafe { msg_send![view, isKindOfClass: IdealystTextField::class()] };
    if !is_ours {
        return;
    }
    // SAFETY: just confirmed the dynamic class is `IdealystTextField`.
    let field: &IdealystTextField = unsafe { &*(view as *const UIView as *const IdealystTextField) };
    field.set_focus_handler(handler);
}

declare_class!(
    /// The `text_area` view: a stock `UITextView` plus `didMoveToWindow` —
    /// the attach hook of the attach-safe `focus()` (`pending_focus`) — the
    /// hardware-Escape key command, and the overlay-responder reclaim on
    /// resign, all as `IdealystTextField` has.
    pub(crate) struct IdealystTextView;

    unsafe impl ClassType for IdealystTextView {
        type Super = UITextView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystTextView";
    }

    impl DeclaredClass for IdealystTextView {
        type Ivars = TextViewIvars;
    }

    unsafe impl IdealystTextView {
        /// Seat the placeholder label over the first line, in the view's
        /// own font, shown only while the view is empty.
        #[method(layoutSubviews)]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            self.layout_placeholder();
        }

        /// A font set by style moves the placeholder with the text.
        #[method(setFont:)]
        fn set_font(&self, font: *mut NSObject) {
            let _: () = unsafe { msg_send![super(self), setFont: font] };
            let _: () = unsafe { msg_send![self, setNeedsLayout] };
        }

        #[method(didMoveToWindow)]
        fn did_move_to_window(&self) {
            let _: () = unsafe { msg_send![super(self), didMoveToWindow] };
            crate::imp::pending_focus::view_moved_to_window(self);
        }

        /// Focus left for nowhere with an overlay open: keep a responder
        /// for its Escape (`keyboard::claim_key_focus`).
        #[method(resignFirstResponder)]
        fn resign_first_responder(&self) -> bool {
            let held: bool = unsafe { msg_send![self, isFirstResponder] };
            let resigned: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            if held && resigned {
                crate::imp::keyboard::claim_key_focus_soon();
            }
            resigned
        }

        #[method_id(keyCommands)]
        fn key_commands(&self) -> Option<Retained<NSObject>> {
            crate::imp::keyboard::escape_key_commands(unsafe { msg_send_id![super(self), keyCommands] })
        }

        #[method(idealystEscapeDismiss:)]
        fn escape_dismiss(&self, _command: *mut NSObject) {
            crate::imp::ffi_guard::guard_ffi("IdealystTextView::escapeDismiss", || {
                crate::imp::keyboard::escape_dismiss();
            });
        }
    }
);

/// UITextView has no placeholder, so the text area draws its own: a label
/// over the first line, created when a placeholder is set.
pub(crate) struct TextViewIvars {
    placeholder: RefCell<Option<Retained<UILabel>>>,
}

impl IdealystTextView {
    /// A new text view, typed as the `UITextView` the backend speaks.
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<UITextView> {
        let this = mtm.alloc::<Self>().set_ivars(TextViewIvars { placeholder: RefCell::new(None) });
        let this: Retained<Self> = unsafe { msg_send_id![super(this), init] };
        Retained::into_super(this)
    }

    /// The placeholder label, made on first use: no taps (they reach the
    /// text view), wrapping, in UIKit's own placeholder gray until an ink
    /// is set.
    fn placeholder_label(&self) -> Retained<UILabel> {
        if let Some(l) = self.ivars().placeholder.borrow().as_ref() {
            return l.clone();
        }
        let mtm = MainThreadMarker::from(self);
        let label: Retained<UILabel> = unsafe { msg_send_id![mtm.alloc::<UILabel>(), init] };
        unsafe {
            let _: () = msg_send![&label, setUserInteractionEnabled: false];
            let _: () = msg_send![&label, setNumberOfLines: 0isize];
            let gray: Retained<UIColor> = msg_send_id![objc2::class!(UIColor), placeholderTextColor];
            let _: () = msg_send![&label, setTextColor: &*gray];
            let _: () = msg_send![self, addSubview: &*label];
        }
        *self.ivars().placeholder.borrow_mut() = Some(label.clone());
        label
    }

    fn layout_placeholder(&self) {
        let Some(label) = self.ivars().placeholder.borrow().clone() else { return };
        unsafe {
            let has_text: bool = msg_send![self, hasText];
            let _: () = msg_send![&label, setHidden: has_text];
            let font: *mut NSObject = msg_send![self, font];
            if !font.is_null() {
                let _: () = msg_send![&label, setFont: font];
            }
            // Where UIKit lays the first glyph: the container inset plus the
            // container's line-fragment padding.
            let inset: TextInsets = msg_send![self, textContainerInset];
            let container: *mut NSObject = msg_send![self, textContainer];
            let pad: CGFloat = if container.is_null() { 0.0 } else { msg_send![container, lineFragmentPadding] };
            let bounds: CGRect = msg_send![self, bounds];
            let width = (bounds.size.width - inset.left - inset.right - 2.0 * pad).max(0.0);
            let fit: CGSize = msg_send![&label, sizeThatFits: CGSize { width, height: CGFloat::MAX }];
            let frame = CGRect {
                origin: CGPoint { x: inset.left + pad, y: inset.top },
                size: CGSize { width, height: fit.height },
            };
            let _: () = msg_send![&label, setFrame: frame];
        }
    }
}

/// Set a text area's placeholder text (`None` removes it).
pub(crate) fn set_text_view_placeholder(view: &UITextView, text: Option<&str>) {
    let Some(tv) = as_idealyst_text_view(view) else { return };
    match text {
        Some(t) => unsafe {
            let label = tv.placeholder_label();
            label.setText(Some(&NSString::from_str(t)));
        },
        None => {
            if let Some(label) = tv.ivars().placeholder.borrow_mut().take() {
                unsafe { label.removeFromSuperview() };
            }
        }
    }
    let _: () = unsafe { msg_send![view, setNeedsLayout] };
}

/// Set a text area's placeholder ink (`None`: UIKit's placeholder gray).
pub(crate) fn set_text_view_placeholder_color(view: &UITextView, color: Option<Retained<UIColor>>) {
    let Some(tv) = as_idealyst_text_view(view) else { return };
    let label = tv.placeholder_label();
    let color = color.unwrap_or_else(|| unsafe { msg_send_id![objc2::class!(UIColor), placeholderTextColor] });
    unsafe { label.setTextColor(Some(&color)) };
}

/// Re-show or hide a text area's placeholder after its text changed.
pub(crate) fn text_view_text_changed(view: &UITextView) {
    let _: () = unsafe { msg_send![view, setNeedsLayout] };
}

fn as_idealyst_text_view(view: &UITextView) -> Option<&IdealystTextView> {
    let is_ours: bool = unsafe { msg_send![view, isKindOfClass: IdealystTextView::class()] };
    // SAFETY: just confirmed the dynamic class is `IdealystTextView`.
    is_ours.then(|| unsafe { &*(view as *const UITextView as *const IdealystTextView) })
}

pub(crate) struct SelectableTextIvars {
    /// The author's padding (`setTextInsets:`). The container inset is
    /// this plus the vertical centring offset, see `layoutSubviews`.
    insets: Cell<TextInsets>,
}

declare_class!(
    /// A selectable `text` (`text(selectable = true)`): a `UITextView`, the
    /// UIKit view with the platform's text selection (long-press, handles,
    /// the edit menu), set up to lay out and draw like the `IdealystLabel`
    /// every other text is:
    ///
    /// - not editable, not scrolling, no line-fragment padding, a clear
    ///   background, no clipping, the label's default font and colour;
    /// - on TextKit 1, the layout a label draws with;
    /// - centred vertically in its bounds as a label centres its text, so
    ///   the glyphs land where the label's would (a measured frame is
    ///   rounded up, leaving a fraction of a point the label splits);
    /// - padding through `setTextInsets:`, as the label's, which becomes
    ///   the `textContainerInset` (plus that centring).
    pub(crate) struct IdealystSelectableText;

    unsafe impl ClassType for IdealystSelectableText {
        type Super = UITextView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystSelectableText";
    }

    impl DeclaredClass for IdealystSelectableText {
        type Ivars = SelectableTextIvars;
    }

    unsafe impl IdealystSelectableText {
        #[method(setTextInsets:)]
        fn set_text_insets(&self, insets: TextInsets) {
            self.ivars().insets.set(insets);
            self.recentre();
        }

        #[method(layoutSubviews)]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            self.recentre();
        }
    }
);

impl IdealystSelectableText {
    /// The author's padding.
    pub(crate) fn author_insets(view: &UITextView) -> TextInsets {
        let this: &Self = unsafe { &*(view as *const UITextView as *const Self) };
        this.ivars().insets.get()
    }

    /// Set the container inset to the author's padding plus half the
    /// height the text leaves unused, as `UILabel` centres its text.
    fn recentre(&self) {
        let author = self.ivars().insets.get();
        let bounds: CGRect = unsafe { msg_send![self, bounds] };
        let used = unsafe {
            let container: *mut NSObject = msg_send![self, textContainer];
            let manager: *mut NSObject = msg_send![self, layoutManager];
            if container.is_null() || manager.is_null() {
                return;
            }
            let _: () = msg_send![manager, ensureLayoutForTextContainer: container];
            let rect: CGRect = msg_send![manager, usedRectForTextContainer: container];
            rect.size.height
        };
        let spare = (bounds.size.height - author.top - author.bottom - used).max(0.0) / 2.0;
        let want = TextInsets { top: author.top + spare, ..author };
        let current: TextInsets = unsafe { msg_send![self, textContainerInset] };
        let same = (current.top - want.top).abs() < 0.001
            && (current.left - want.left).abs() < 0.001
            && (current.bottom - want.bottom).abs() < 0.001
            && (current.right - want.right).abs() < 0.001;
        if !same {
            let _: () = unsafe { msg_send![self, setTextContainerInset: want] };
        }
    }
}

impl IdealystSelectableText {
    /// A new selectable text, typed as the `UITextView` the backend speaks.
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<UITextView> {
        let this = mtm.alloc::<Self>().set_ivars(SelectableTextIvars { insets: Cell::new(TextInsets::default()) });
        let this: Retained<Self> = unsafe { msg_send_id![super(this), init] };
        let view: Retained<UITextView> = Retained::into_super(this);
        unsafe {
            // Reading `layoutManager` puts the view on TextKit 1, the layout
            // a `UILabel` draws with. On TextKit 2 (the default) the glyphs
            // sat half a point off the label's.
            let _: *mut NSObject = msg_send![&view, layoutManager];
            let _: () = msg_send![&view, setEditable: false];
            let _: () = msg_send![&view, setSelectable: true];
            let _: () = msg_send![&view, setScrollEnabled: false];
            let _: () = msg_send![&view, setTextContainerInset: TextInsets::default()];
            let container: *mut objc2::runtime::AnyObject = msg_send![&view, textContainer];
            if !container.is_null() {
                let _: () = msg_send![container, setLineFragmentPadding: 0.0 as CGFloat];
                // Word wrap, no line limit: the label's create-time
                // configuration (`apply_label_line_config` with `None`).
                let _: () = msg_send![container, setLineBreakMode: 0isize];
                let _: () = msg_send![container, setMaximumNumberOfLines: 0usize];
            }
            // A text view's natural font is 12pt and its colour black; a
            // label's are the system 17pt and `labelColor`. Take them from
            // a real label, so a text that no style touches renders as one.
            let label: Retained<UILabel> = msg_send_id![mtm.alloc::<UILabel>(), init];
            let font: *mut NSObject = msg_send![&label, font];
            let color: *mut NSObject = msg_send![&label, textColor];
            let _: () = msg_send![&view, setFont: font];
            let _: () = msg_send![&view, setTextColor: color];
            let clear: Retained<objc2_ui_kit::UIColor> = objc2_ui_kit::UIColor::clearColor();
            let _: () = msg_send![&view, setBackgroundColor: &*clear];
            // A label doesn't clip (glyph overhang, a text shadow); a text
            // view does by default, and `apply_clip` keeps the natural value.
            let _: () = msg_send![&view, setClipsToBounds: false];
        }
        view
    }
}

/// The size a selectable text needs at `avail_w` points wide (∞ for
/// unbounded), for its Taffy measure_fn: what the label's `sizeThatFits:`
/// answers — the widest laid-out line, the height of the lines shown, plus
/// the author's padding. A text view's own `sizeThatFits:` answers the
/// width it is given, so the text is laid out in a scratch TextKit 1 stack
/// (the layout the view draws with: same string, line limit and wrap mode)
/// at that width instead. The centring offset is not padding and is left
/// out.
pub(crate) fn selectable_text_fit(view: &UITextView, avail_w: f32) -> CGSize {
    let insets = IdealystSelectableText::author_insets(view);
    let pad_w = insets.left + insets.right;
    let pad_h = insets.top + insets.bottom;
    let bound = if avail_w.is_finite() {
        (avail_w as CGFloat - pad_w).max(0.0)
    } else {
        CGFloat::MAX
    };
    let used = unsafe {
        let attributed: *mut NSObject = msg_send![view, attributedText];
        let live: *mut NSObject = msg_send![view, textContainer];
        let storage: Retained<NSObject> = msg_send_id![
            msg_send_id![objc2::class!(NSTextStorage), alloc],
            initWithAttributedString: attributed
        ];
        let manager: Retained<NSObject> = msg_send_id![objc2::class!(NSLayoutManager), new];
        let container: Retained<NSObject> = msg_send_id![
            msg_send_id![objc2::class!(NSTextContainer), alloc],
            initWithSize: CGSize { width: bound, height: CGFloat::MAX }
        ];
        let _: () = msg_send![&container, setLineFragmentPadding: 0.0 as CGFloat];
        if !live.is_null() {
            let lines: usize = msg_send![live, maximumNumberOfLines];
            let mode: isize = msg_send![live, lineBreakMode];
            let _: () = msg_send![&container, setMaximumNumberOfLines: lines];
            let _: () = msg_send![&container, setLineBreakMode: mode];
        }
        let _: () = msg_send![&manager, addTextContainer: &*container];
        let _: () = msg_send![&storage, addLayoutManager: &*manager];
        let _: () = msg_send![&manager, ensureLayoutForTextContainer: &*container];
        let rect: CGRect = msg_send![&manager, usedRectForTextContainer: &*container];
        rect.size
    };
    CGSize { width: used.width.min(bound) + pad_w, height: used.height + pad_h }
}
