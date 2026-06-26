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
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker};
use objc2_ui_kit::{UILabel, UITextField, UIView};

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
    focus_setter: RefCell<Option<Rc<dyn Fn(bool)>>>,
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

        /// Focus gained → flip FOCUSED on (the focus ring).
        #[method(becomeFirstResponder)]
        fn become_first_responder(&self) -> bool {
            let became: bool = unsafe { msg_send![super(self), becomeFirstResponder] };
            if became {
                self.fire_focus(true);
            }
            became
        }

        /// Focus lost → flip FOCUSED off.
        #[method(resignFirstResponder)]
        fn resign_first_responder(&self) -> bool {
            let resigned: bool = unsafe { msg_send![super(self), resignFirstResponder] };
            self.fire_focus(false);
            resigned
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
        });
        unsafe { msg_send_id![super(this), init] }
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

    /// Install the focus-state setter (from `Backend::attach_states`).
    pub(crate) fn set_focus_setter(&self, setter: Rc<dyn Fn(bool)>) {
        *self.ivars().focus_setter.borrow_mut() = Some(setter);
    }

    fn fire_focus(&self, on: bool) {
        let cb = self.ivars().focus_setter.borrow().clone();
        if let Some(cb) = cb {
            cb(on);
        }
    }
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
