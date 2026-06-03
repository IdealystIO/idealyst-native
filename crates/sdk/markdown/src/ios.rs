//! iOS handler for the `markdown` external.
//!
//! Renders the WHOLE document as ONE `UILabel` whose `attributedText` is
//! an `NSAttributedString` built from the shared [`segments::lower`]
//! flattening. Each segment becomes a fragment carrying its own
//! attribute dictionary (foreground color, font with size + bold/italic/
//! monospace traits, optional background tint, underline, strikethrough)
//! — one fragment append per uniform run, zero per-run native widgets.
//!
//! The label wraps to the column width via the iOS backend's width-aware
//! external measure (`install_external_wrapping_measure`): a plain
//! `UIScrollView`-style probe would report single-line height and clip
//! the document. See [`crate`] docs for the perf rationale.

use crate::ir::MarkdownDoc;
use crate::segments::{self, Seg};
use backend_ios::{IosBackend, IosNode};
use backend_ios_core::style::color_to_uicolor;
use objc2::rc::{Allocated, Retained};
use objc2::runtime::NSObject;
use objc2::{class, msg_send, msg_send_id};
use objc2_foundation::{NSAttributedString, NSMutableAttributedString, NSString};
use objc2_ui_kit::UILabel;
use runtime_core::Color;
use std::rc::Rc;

/// `NSLineBreakByWordWrapping`.
const LINE_BREAK_WORD_WRAP: isize = 0;
/// `UIFontDescriptorSymbolicTraits`: italic = 1<<0, bold = 1<<1.
///
/// MUST be `u32` (objc type code 'I'), NOT `usize`/'Q': on iOS
/// `UIFontDescriptorSymbolicTraits` is `uint32_t`, and passing a 64-bit
/// value to `-fontDescriptorWithSymbolicTraits:` aborts with an
/// "invalid message send … expected 'I', found 'Q'" type-encoding error.
const TRAIT_ITALIC: u32 = 1 << 0;
const TRAIT_BOLD: u32 = 1 << 1;
/// `UIFontWeightBold` ≈ 0.4 (used for the monospace bold path).
const WEIGHT_BOLD: f64 = 0.4;
const WEIGHT_REGULAR: f64 = 0.0;

/// `Element::External` handler: one `UILabel` carrying the whole doc.
pub(crate) fn build(doc: &Rc<MarkdownDoc>, backend: &mut IosBackend) -> IosNode {
    let mtm = backend.mtm();
    let label: Retained<UILabel> = unsafe { UILabel::new(mtm) };
    // Multi-line, word-wrapped.
    let _: () = unsafe { msg_send![&label, setNumberOfLines: 0isize] };
    let _: () = unsafe { msg_send![&label, setLineBreakMode: LINE_BREAK_WORD_WRAP] };

    let segs = segments::lower(doc);
    let attributed = build_attributed(&segs);
    let _: () = unsafe {
        msg_send![&label, setAttributedText: &*attributed as &NSAttributedString]
    };

    // Width-aware measure so the label wraps to the parent column and
    // reports its true multi-line height (no padding — the call site
    // owns outer spacing via `.with_style`).
    let view = label_as_view(&label);
    backend.install_external_wrapping_measure(view, view, 0.0);

    IosNode::Label(label)
}

/// Reborrow a `UILabel` as its `UIView` superclass for the measure call.
fn label_as_view(label: &Retained<UILabel>) -> &objc2_ui_kit::UIView {
    // UILabel: UIView; Deref chains down to UIView.
    label
}

/// Build the document `NSMutableAttributedString` by appending one
/// attributed fragment per segment.
fn build_attributed(segs: &[Seg]) -> Retained<NSMutableAttributedString> {
    let attributed: Retained<NSMutableAttributedString> =
        unsafe { msg_send_id![class!(NSMutableAttributedString), new] };
    for seg in segs {
        let ns_text = NSString::from_str(&seg.text);
        let dict = build_attr_dict(seg);
        let fragment: Retained<NSAttributedString> = unsafe {
            let alloc: Allocated<NSAttributedString> = msg_send_id![class!(NSAttributedString), alloc];
            msg_send_id![alloc, initWithString: &*ns_text, attributes: &*dict]
        };
        let _: () = unsafe {
            msg_send![&*attributed, appendAttributedString: &*fragment]
        };
    }
    attributed
}

/// Build the `@{...}` attribute dictionary for one segment. Keys are the
/// documented attribute-name constants, hard-coded to their literal
/// string values (codeblock does the same to avoid `@class` linkage):
/// `NSColor`, `NSFont`, `NSBackgroundColor`, `NSUnderline`,
/// `NSStrikethrough`.
fn build_attr_dict(seg: &Seg) -> Retained<NSObject> {
    let dict: Retained<NSObject> =
        unsafe { msg_send_id![class!(NSMutableDictionary), dictionary] };

    let color = color_to_uicolor(&Color(seg.style.color.clone()));
    set_obj(&dict, "NSColor", &color);

    let font = make_font(seg.style.size as f64, seg.style.bold, seg.style.italic, seg.style.mono);
    set_obj(&dict, "NSFont", &font);

    if let Some(bg) = &seg.style.bg {
        let bgc = color_to_uicolor(&Color(bg.clone()));
        set_obj(&dict, "NSBackgroundColor", &bgc);
    }
    if seg.style.underline {
        set_obj(&dict, "NSUnderline", &*number(1));
    }
    if seg.style.strike {
        set_obj(&dict, "NSStrikethrough", &*number(1));
    }
    dict
}

fn set_obj(dict: &Retained<NSObject>, key: &str, value: &NSObject) {
    let ns_key = NSString::from_str(key);
    let _: () = unsafe { msg_send![&**dict, setObject: value, forKey: &*ns_key] };
}

/// `[NSNumber numberWithInteger:]`.
fn number(v: isize) -> Retained<NSObject> {
    unsafe { msg_send_id![class!(NSNumber), numberWithInteger: v] }
}

/// Build a `UIFont` for the given size + traits.
///
/// - monospace → `monospacedSystemFontOfSize:weight:`.
/// - bold+italic → systemFont descriptor with both symbolic traits.
/// - bold / italic → `boldSystemFontOfSize:` / `italicSystemFontOfSize:`.
/// - else → `systemFontOfSize:`.
fn make_font(size: f64, bold: bool, italic: bool, mono: bool) -> Retained<NSObject> {
    let cls = class!(UIFont);
    unsafe {
        if mono {
            let weight = if bold { WEIGHT_BOLD } else { WEIGHT_REGULAR };
            let f: Option<Retained<NSObject>> =
                msg_send_id![cls, monospacedSystemFontOfSize: size, weight: weight];
            if let Some(f) = f {
                return f;
            }
        }
        if bold && italic {
            let base: Retained<NSObject> = msg_send_id![cls, systemFontOfSize: size];
            let desc: Retained<NSObject> = msg_send_id![&base, fontDescriptor];
            let traits: u32 = TRAIT_BOLD | TRAIT_ITALIC;
            let new_desc: Option<Retained<NSObject>> =
                msg_send_id![&desc, fontDescriptorWithSymbolicTraits: traits];
            if let Some(nd) = new_desc {
                // size 0.0 = keep the descriptor's size (which inherits
                // from the system font we built it off).
                let f: Retained<NSObject> = msg_send_id![cls, fontWithDescriptor: &*nd, size: 0.0f64];
                return f;
            }
            return msg_send_id![cls, boldSystemFontOfSize: size];
        }
        if bold {
            return msg_send_id![cls, boldSystemFontOfSize: size];
        }
        if italic {
            return msg_send_id![cls, italicSystemFontOfSize: size];
        }
        msg_send_id![cls, systemFontOfSize: size]
    }
}
