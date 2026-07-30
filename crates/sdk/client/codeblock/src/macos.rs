//! macOS handler for the `code_block` external. Produces a single
//! `NSScrollView` (horizontal) wrapping an `NSTextField` label in label mode
//! whose `attributedStringValue` is an `NSAttributedString` with per-run
//! `NSForegroundColorAttributeName` ranges. One label per code block,
//! regardless of token count.
//!
//! Mirrors the iOS handler (`UIScrollView` + `UILabel` + `NSAttributedString`)
//! and the Android one (`HorizontalScrollView` + `TextView` + `SpannableString`):
//! every per-token color lowers to an inline attribute on a single native text
//! widget instead of one widget per token. Long lines scroll horizontally
//! rather than wrapping (single-axis scroller), matching `<pre>{overflow-x:auto}`.

use backend_macos::{MacosBackend, MacosNode};
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyObject, NSObject};
use objc2::{msg_send, msg_send_id};
use objc2_app_kit::{NSColor, NSTextField, NSView};
use objc2_foundation::{
    CGPoint, CGRect, CGSize, MainThreadMarker, NSAttributedString, NSDictionary,
    NSMutableAttributedString, NSString,
};
use runtime_shared::Color;

/// Build the macOS codeblock node. Returns the wrapping `NSScrollView` so
/// the framework parents it into the surrounding view tree; the inner
/// `NSTextField` + `NSAttributedString` are invisible to Taffy (one
/// node, one layout entry sized via
/// `install_external_content_measure`).
pub(crate) fn build(spans: &[(String, Color)], backend: &mut MacosBackend) -> MacosNode {
    let mtm = backend.mtm();

    // Label in label mode: non-editable, non-selectable, transparent. Multi-line
    // (so `\n`s break) but NOT wrapping — the scroll view reveals long lines.
    let empty = NSString::from_str("");
    let label: Retained<NSTextField> =
        unsafe { msg_send_id![objc2::class!(NSTextField), labelWithString: &*empty] };
    let cell: Retained<NSObject> = unsafe { msg_send_id![&label, cell] };
    unsafe {
        let _: () = msg_send![&cell, setWraps: false];
        let _: () = msg_send![&cell, setUsesSingleLineMode: false];
        // NSLineBreakByClipping (= 2): keep long lines intact; the scroll view
        // exposes the overflow instead of truncating with an ellipsis.
        let _: () = msg_send![&cell, setLineBreakMode: 2u64];
    }
    set_monospace_font(&label, 13.0);

    // Attributed text: one `NSForegroundColorAttributeName` range per run.
    let attributed = build_attributed_string(spans, mtm);
    unsafe {
        let _: () = msg_send![
            &label,
            setAttributedStringValue: &*attributed as &NSAttributedString
        ];
        // Size the label to its content so the scroll view's documentView has a
        // real frame; the framework never lays the label out (it's the
        // documentView, not a Taffy node).
        let _: () = msg_send![&label, sizeToFit];
    }

    // The label sits inside a CONTAINER documentView, NOT via
    // `NSScrollView.contentInsets`: contentInsets shift the overlay
    // scroller up with the content, so the horizontal scrollbar floats over
    // the text instead of sitting at the box's bottom edge. The handler
    // applies NO padding of its own — the author's `padding_*` (via
    // `.with_style` on the `code_block`) lands on the scroll node, and the
    // backend diverts it to the label's offset inside this container (see
    // `MacosBackend::sync_external_scroller_padding`), so the padding
    // scrolls WITH the content and mid-scroll text reaches the block's own
    // edge — the web `<pre> { padding }` model. A previous revision baked a
    // 20pt inset here; combined with author padding on a wrapping panel
    // that doubled the visible padding.
    let label_frame: CGRect = unsafe { msg_send![&label, frame] };
    let container_rect = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: label_frame.size,
    };
    let container: Retained<NSView> = unsafe {
        let alloc: *mut AnyObject = msg_send![objc2::class!(NSView), alloc];
        let inited: *mut AnyObject = msg_send![alloc, initWithFrame: container_rect];
        Retained::from_raw(inited.cast::<NSView>()).expect("NSView init returned nil")
    };
    unsafe {
        let origin_frame = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: label_frame.size,
        };
        let _: () = msg_send![&label, setFrame: origin_frame];
        let _: () = msg_send![&container, addSubview: &*label];
    }

    // Horizontal-only `NSScrollView`. `setDrawsBackground: false` so the
    // surrounding `CodePanel` background shows through.
    let scroll: Retained<NSView> = unsafe {
        let alloc: *mut AnyObject = msg_send![objc2::class!(NSScrollView), alloc];
        let zero = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width: 0.0, height: 0.0 },
        };
        let inited: *mut AnyObject = msg_send![alloc, initWithFrame: zero];
        Retained::from_raw(inited.cast::<NSView>()).expect("NSScrollView init returned nil")
    };
    unsafe {
        let _: () = msg_send![&scroll, setHasHorizontalScroller: true];
        let _: () = msg_send![&scroll, setHasVerticalScroller: false];
        let _: () = msg_send![&scroll, setAutohidesScrollers: true];
        let _: () = msg_send![&scroll, setDrawsBackground: false];
        let _: () = msg_send![&scroll, setDocumentView: &*container];
    }

    // Register + measure: a bare NSScrollView has no intrinsic size, so without
    // this it collapses to 0×0 in the flex column and the codeblock renders
    // blank. The measure fills the parent's width (scrolling content wider than
    // it) and reports the label's content height + the author-styled insets.
    // This also registers the node for the padding divert described above.
    let label_view: &NSView = &label;
    backend.install_external_content_measure(&scroll, label_view);

    MacosNode::View(scroll)
}

/// Build an `NSMutableAttributedString`, appending each `(text, color)` run and
/// applying `NSForegroundColorAttributeName` to its range. One range per run;
/// the field's text engine paints inline with no per-run layout cost. Mirrors
/// the iOS handler's `build_attributed_string`.
fn build_attributed_string(
    spans: &[(String, Color)],
    _mtm: MainThreadMarker,
) -> Retained<NSMutableAttributedString> {
    let attributed: Retained<NSMutableAttributedString> =
        unsafe { msg_send_id![objc2::class!(NSMutableAttributedString), new] };
    for (text, color) in spans {
        let ns_text = NSString::from_str(text);
        let ns_color = color_to_nscolor(color);
        let attrs = build_color_dict(&ns_color);
        let fragment: Retained<NSAttributedString> = unsafe {
            let alloc: Allocated<NSAttributedString> =
                msg_send_id![objc2::class!(NSAttributedString), alloc];
            msg_send_id![alloc, initWithString: &*ns_text, attributes: &*attrs]
        };
        let _: () = unsafe { msg_send![&attributed, appendAttributedString: &*fragment] };
    }
    attributed
}

/// `@{NSForegroundColorAttributeName: color}` via
/// `[NSDictionary dictionaryWithObject:forKey:]`. The key constant resolves to
/// the literal `@"NSColor"` on macOS (same as iOS); hard-coded to avoid a
/// `@class` linker dependency on the imported constant.
fn build_color_dict(color: &NSColor) -> Retained<NSDictionary<NSString, NSObject>> {
    let key = NSString::from_str("NSColor");
    let color_obj: &NSObject = unsafe { &*(color as *const NSColor as *const NSObject) };
    unsafe {
        msg_send_id![
            objc2::class!(NSDictionary),
            dictionaryWithObject: color_obj,
            forKey: &*key,
        ]
    }
}

/// Parse the framework `Color` (a CSS-ish string) and build an sRGB `NSColor`.
/// We resolve via `runtime_shared::color` rather than the backend's private
/// helper so the SDK stays decoupled from `backend-macos` internals.
fn color_to_nscolor(color: &Color) -> Retained<NSColor> {
    let rgba = runtime_shared::color::parse_or(&color.0, runtime_shared::color::Rgba::BLACK);
    unsafe {
        NSColor::colorWithSRGBRed_green_blue_alpha(
            rgba.r as f64 / 255.0,
            rgba.g as f64 / 255.0,
            rgba.b as f64 / 255.0,
            rgba.a as f64 / 255.0,
        )
    }
}

/// `+[NSFont monospacedSystemFontOfSize:weight:]` (macOS 10.15+). Falls back to
/// Menlo by name if unavailable. Mirrors the iOS handler's `set_monospace_font`.
fn set_monospace_font(label: &NSTextField, size: f64) {
    unsafe {
        let cls = objc2::class!(NSFont);
        // weight 0.0 = NSFontWeightRegular
        let font: Option<Retained<NSObject>> =
            msg_send_id![cls, monospacedSystemFontOfSize: size, weight: 0.0_f64];
        if let Some(f) = font {
            let _: () = msg_send![label, setFont: &*f];
            return;
        }
        let menlo = NSString::from_str("Menlo");
        let font: Option<Retained<NSObject>> =
            msg_send_id![cls, fontWithName: &*menlo, size: size];
        if let Some(f) = font {
            let _: () = msg_send![label, setFont: &*f];
        }
    }
}
