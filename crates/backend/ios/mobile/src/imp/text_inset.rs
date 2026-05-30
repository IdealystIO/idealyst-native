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

use std::cell::Cell;

use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGRect, CGSize, MainThreadMarker};
use objc2_ui_kit::UILabel;

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
