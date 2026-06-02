//! iOS handler for the `code_block` external. Produces a single
//! `UIScrollView` (horizontal) wrapping one `UILabel` whose
//! `attributedText` is an `NSAttributedString` with per-range
//! `NSForegroundColorAttributeName` attributes. One label per code
//! block, regardless of token count.
//!
//! Mirrors the Android handler (HorizontalScrollView + TextView +
//! SpannableString). Both share the rationale: every per-token span
//! lowers to an inline attribute on a single native text widget
//! instead of one widget per token.

use crate::CodeBlockProps;
use backend_ios::{IosBackend, IosNode};
use backend_ios_core::style::color_to_uicolor;
use objc2::runtime::NSObject;
use objc2::{msg_send, msg_send_id};
use objc2::rc::{Allocated, Retained};
use objc2_foundation::{
    NSAttributedString, NSDictionary, NSMutableAttributedString, NSString,
};
use objc2_ui_kit::{UILabel, UIScrollView};
use runtime_core::Color;
use std::rc::Rc;

/// Inner padding (in points) drawn around the code text. Matches the
/// Android handler's `RustCodeBlock.PADDING_DP` and the web's
/// canonical `<pre>` padding. Picked as a single constant here so the
/// look stays consistent if the framework ever exposes a per-instance
/// override.
const PADDING_PT: f64 = 20.0;

/// Mirror of UIKit's `UIEdgeInsets` struct. We don't pull the full
/// `objc2-ui-kit` type because the SDK's tiny feature set
/// (UIScrollView + UILabel) doesn't already enable it — re-declaring
/// keeps `Cargo.toml` minimal. Same trick `virtualizer.rs` uses.
#[repr(C)]
#[derive(Clone, Copy)]
struct UIEdgeInsets {
    top: f64,
    left: f64,
    bottom: f64,
    right: f64,
}
unsafe impl objc2::Encode for UIEdgeInsets {
    const ENCODING: objc2::Encoding = objc2::Encoding::Struct(
        "UIEdgeInsets",
        &[
            <f64 as objc2::Encode>::ENCODING,
            <f64 as objc2::Encode>::ENCODING,
            <f64 as objc2::Encode>::ENCODING,
            <f64 as objc2::Encode>::ENCODING,
        ],
    );
}

fn set_content_inset(scroll: &UIScrollView, pad: f64) {
    let insets = UIEdgeInsets {
        top: pad,
        left: pad,
        bottom: pad,
        right: pad,
    };
    let _: () = unsafe { msg_send![scroll, setContentInset: insets] };
}

/// `Element::External` handler for the iOS codeblock kind. Returns
/// the wrapping UIScrollView so the framework parents it into the
/// surrounding view tree; the inner UILabel + NSAttributedString are
/// invisible to Taffy / the layout pass (one external node, one
/// layout entry).
pub(crate) fn build(props: &Rc<CodeBlockProps>, backend: &mut IosBackend) -> IosNode {
    let mtm = backend.mtm();

    // Build the label. UILabel auto-handles multi-line text when
    // `numberOfLines = 0` and the attributed string contains `\n`s.
    let label: Retained<UILabel> = unsafe { UILabel::new(mtm) };
    let _: () = unsafe { msg_send![&label, setNumberOfLines: 0isize] };
    // Monospace font. UIFont's `monospacedSystemFont` is the right
    // call on iOS 13+; that's well below our deployment floor.
    set_monospace_font(&label, 13.0);

    // Compose the attributed text from all spans. Cast the
    // NSMutableAttributedString down to its NSAttributedString
    // superclass for setAttributedText: — UILabel doesn't care
    // which subclass we hand it.
    let attributed = build_attributed_string(&props.spans);
    let _: () = unsafe {
        msg_send![&label, setAttributedText: &*attributed as &NSAttributedString]
    };

    // Wrap the label in a horizontal-only scroll view so over-wide
    // lines scroll left-right instead of wrapping. Same scroll
    // behavior the Android backend gets from HorizontalScrollView.
    let scroll: Retained<UIScrollView> = unsafe { UIScrollView::new(mtm) };
    // Horizontal-only scroll: enable horizontal-scroll, no vertical
    // bounce — the caller can wrap our scroll view in their own
    // vertical scroll view when they want both axes (matches the
    // framework's single-axis `scroll_view` primitive).
    let _: () = unsafe { msg_send![&scroll, setAlwaysBounceHorizontal: true] };
    let _: () = unsafe { msg_send![&scroll, setAlwaysBounceVertical: false] };
    let _: () = unsafe { msg_send![&scroll, setShowsHorizontalScrollIndicator: false] };
    let _: () = unsafe { msg_send![&scroll, setShowsVerticalScrollIndicator: false] };

    // Inner padding via `contentInset` — UIScrollView treats this as
    // extra scrollable space around the content, so the user can
    // scroll past the content edge to reveal the inset. Visual
    // result: the code text always has 20pt of breathing room on
    // each side, even when scrolled to either horizontal extreme
    // (matching `<pre> { padding: 20px; overflow-x: auto }` on web
    // and the Android handler's `setPadding` on the inner TextView).
    //
    // UILabel doesn't honor a setPadding-style API natively (no
    // textInsets without a UILabel subclass); contentInset on the
    // wrapping scroll view is the conventional iOS approach for
    // this exact pattern.
    //
    // Also disable contentInset adjustment so the system doesn't
    // layer safe-area insets on top of ours (iOS 11+ default behavior
    // would add ~20pt extra below the status bar via "Automatic",
    // which would double-count our top inset).
    set_content_inset(&scroll, PADDING_PT);
    let _: () = unsafe {
        // UIScrollViewContentInsetAdjustmentBehavior.never = 2
        msg_send![&scroll, setContentInsetAdjustmentBehavior: 2isize]
    };

    // Add the label as the scroll view's only subview. UIScrollView
    // sizes its `contentSize` from its subviews' frame extents — the
    // framework's iOS layout pass already handles that via
    // `scroll_views` registration. We register the scroll view here
    // so the framework picks it up.
    unsafe { scroll.addSubview(&label) };

    // Register the scroll view with the framework's Taffy layout tree
    // AND give it a measure_fn driven by the label's `sizeThatFits:` — a
    // bare `UIScrollView` has no intrinsic size, so without this it
    // collapses to 0×0 in a flex column (the parent `CodePanel` sets no
    // height) and the codeblock renders blank. `PADDING_PT` on each side
    // matches the `contentInset` set above so the box includes the same
    // breathing room the content scrolls within.
    backend.install_external_content_measure(&scroll, &label, PADDING_PT as f32);

    // We need the label's intrinsic size to actually drive the
    // scroll view's contentSize. Without a layout pass, UILabel
    // sits at its initial frame (0×0). Force a `sizeToFit` now so
    // the framework's `scroll_contentsize_sync` (run after every
    // layout pass) sees the right frame.
    let _: () = unsafe { msg_send![&label, sizeToFit] };

    IosNode::ScrollView(scroll)
}

/// Builds an `NSMutableAttributedString` by appending each
/// `(text, color)` span and applying `NSForegroundColorAttributeName`
/// to its range. One range per span; the label's text engine paints
/// inline with no per-span layout cost.
///
/// We construct each fragment via `[NSAttributedString alloc]` +
/// `initWithString:attributes:` (the documented two-arg initializer)
/// because `objc2-foundation`'s safe wrapper for that initializer
/// gates on traits we don't pull in. The msg_send_id macro picks the
/// right retain semantics for an init method.
fn build_attributed_string(
    spans: &[(String, Color)],
) -> Retained<NSMutableAttributedString> {
    let attributed: Retained<NSMutableAttributedString> = unsafe {
        let cls = objc2::class!(NSMutableAttributedString);
        msg_send_id![cls, new]
    };
    for (text, color) in spans {
        let ns_text = NSString::from_str(text);
        let ui_color = color_to_uicolor(color);
        let attrs_dict = build_color_dict(&*ui_color);
        let fragment: Retained<NSAttributedString> = unsafe {
            let cls = objc2::class!(NSAttributedString);
            let alloc: Allocated<NSAttributedString> = msg_send_id![cls, alloc];
            msg_send_id![
                alloc,
                initWithString: &*ns_text,
                attributes: &*attrs_dict,
            ]
        };
        let _: () = unsafe {
            msg_send![&*attributed, appendAttributedString: &*fragment]
        };
    }
    attributed
}

/// Build `@{NSForegroundColorAttributeName: color}` via
/// `[NSDictionary dictionaryWithObject:forKey:]`. The key string is
/// the UIKit-documented `NSForegroundColorAttributeName`, which
/// resolves to the literal `@"NSColor"` constant on iOS / macOS.
/// We hard-code the string here to avoid a `@class` linker
/// dependency on the imported constant.
fn build_color_dict(color: &NSObject) -> Retained<NSDictionary<NSString, NSObject>> {
    let key = NSString::from_str("NSColor");
    unsafe {
        let cls = objc2::class!(NSDictionary);
        msg_send_id![
            cls,
            dictionaryWithObject: color,
            forKey: &*key,
        ]
    }
}

/// `UIFont.monospacedSystemFontOfSize:weight:` on iOS 13+. Falls
/// back to a Menlo lookup if the API isn't available; on any modern
/// device the primary path always succeeds.
fn set_monospace_font(label: &UILabel, size: f64) {
    unsafe {
        let cls = objc2::class!(UIFont);
        // weight = UIFontWeightRegular = 0.0
        let font: Option<Retained<NSObject>> = msg_send_id![
            cls,
            monospacedSystemFontOfSize: size,
            weight: 0.0_f64,
        ];
        if let Some(f) = font {
            let _: () = msg_send![label, setFont: &*f];
            return;
        }
        // Fallback: ask UIKit for the Menlo face by name.
        let ns = NSString::from_str("Menlo");
        let font: Option<Retained<NSObject>> = msg_send_id![
            cls,
            fontWithName: &*ns,
            size: size,
        ];
        if let Some(f) = font {
            let _: () = msg_send![label, setFont: &*f];
        }
    }
}
