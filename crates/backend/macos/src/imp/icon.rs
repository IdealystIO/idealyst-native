//! `Element::Icon` — vector icon rendering on macOS via
//! `CAShapeLayer` driven by a `CGPath` built from the framework's
//! `IconData.paths`.
//!
//! Path parsing lives in [`backend_apple_core::icon_path`] — shared
//! with the iOS backend, no duplication. This module is the
//! macOS-specific adapter: a `PathEmitter` impl that writes into a
//! `CGMutablePathRef` via raw CoreGraphics FFI, plus the NSView +
//! CAShapeLayer mount.
//!
//! ## Why CGPath instead of NSBezierPath
//!
//! `CAShapeLayer.path` takes a `CGPathRef`. NSBezierPath only
//! exposes its `cgPath` getter on macOS 14+; before that we'd
//! have to walk elements manually. CGPath via the C API works on
//! every macOS version we support and matches what iOS's
//! UIBezierPath does internally (UIBezierPath is a CGPath wrapper).
//!
//! ## Stroke vs fill
//!
//! Mirror iOS's `create_icon`: outlined icons (`IconData.filled ==
//! false`) stroke with rounded caps and line width = 2 × scale factor;
//! filled icons (`filled == true`) fill the path with the icon color
//! and disable the stroke. `update_icon_color` keys off the layer's
//! live `fillColor` alpha to know which paint to rewrite.

use std::ffi::c_void;

use objc2::encode::{Encoding, RefEncode};
use objc2::class;
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2_app_kit::{NSColor, NSView};
use objc2_foundation::{CGPoint, CGRect, CGSize, NSObject, NSString};
use runtime_core::primitives::icon::{FillRule, IconData};
use runtime_core::Color;

use backend_apple_core::icon_path::{parse_svg_path, PathEmitter};

use super::{color_to_nscolor, FlippedView, MacosNode};

// =========================================================================
// CoreGraphics FFI — cross-Apple path-construction primitives. CGPath
// is opaque; we never dereference the pointer ourselves. CGFloat is
// f64 on every 64-bit Apple platform.
// =========================================================================

#[repr(C)]
struct CGPathRef(c_void);

// `-[CAShapeLayer setPath:]` wants a typed `CGPathRef` (`^{CGPath=}`). Without
// this, passing the pointer as `*const c_void` (`^v`) trips objc2's encoding
// verifier and SIGABRTs (same family as the CGContext/CGColor traps —
// [[project_macos_appkit_uikit_diffs]] gotchas #2/#9). `RefEncode` makes
// `*mut CGPathRef` carry the right `^{CGPath=}` encoding through `msg_send!`.
unsafe impl RefEncode for CGPathRef {
    const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct("CGPath", &[]));
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPathCreateMutable() -> *mut CGPathRef;
    fn CGPathMoveToPoint(
        path: *mut CGPathRef,
        m: *const c_void,
        x: f64,
        y: f64,
    );
    fn CGPathAddLineToPoint(
        path: *mut CGPathRef,
        m: *const c_void,
        x: f64,
        y: f64,
    );
    fn CGPathAddCurveToPoint(
        path: *mut CGPathRef,
        m: *const c_void,
        cp1x: f64,
        cp1y: f64,
        cp2x: f64,
        cp2y: f64,
        x: f64,
        y: f64,
    );
    fn CGPathCloseSubpath(path: *mut CGPathRef);
    fn CGPathRelease(path: *mut CGPathRef);
    fn CGColorGetAlpha(color: *const c_void) -> f64;
}

/// `PathEmitter` adapter that writes into a CGMutablePathRef.
/// Owns the path; transfers ownership via `into_raw` so the
/// CAShapeLayer can take a `+0`-retained reference and Core
/// Animation manages the lifecycle through its own retain.
struct CGPathEmitter {
    path: *mut CGPathRef,
}

impl CGPathEmitter {
    fn new() -> Self {
        Self {
            path: unsafe { CGPathCreateMutable() },
        }
    }

    /// Take ownership of the raw CGPath pointer. Caller is
    /// responsible for releasing — typically by handing it to
    /// `CAShapeLayer.setPath:` which retains and then releasing
    /// the local +1 reference via `CGPathRelease`.
    fn into_raw(self) -> *mut CGPathRef {
        let p = self.path;
        std::mem::forget(self);
        p
    }
}

impl Drop for CGPathEmitter {
    fn drop(&mut self) {
        if !self.path.is_null() {
            unsafe { CGPathRelease(self.path) };
        }
    }
}

impl PathEmitter for CGPathEmitter {
    fn move_to(&mut self, x: f64, y: f64) {
        unsafe { CGPathMoveToPoint(self.path, std::ptr::null(), x, y) };
    }

    fn line_to(&mut self, x: f64, y: f64) {
        unsafe { CGPathAddLineToPoint(self.path, std::ptr::null(), x, y) };
    }

    fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        unsafe {
            CGPathAddCurveToPoint(self.path, std::ptr::null(), c1x, c1y, c2x, c2y, x, y)
        };
    }

    fn close(&mut self) {
        unsafe { CGPathCloseSubpath(self.path) };
    }
}

// =========================================================================
// create_icon — public entry point invoked from MacosBackend.
// =========================================================================

/// Build a layer-backed `NSView` whose `CAShapeLayer` sublayer
/// renders `data.paths` in `color`. Matches the iOS create_icon
/// shape (24×24 default, stroke-only with rounded caps, line width
/// 2× scale factor).
pub(crate) fn create_icon(
    mtm: objc2_foundation::MainThreadMarker,
    data: &IconData,
    color: Option<&Color>,
) -> Retained<NSView> {
    const SIZE: f64 = 24.0;
    let (vw, vh) = data.view_box;
    let sx = SIZE / vw as f64;
    let sy = SIZE / vh as f64;

    let view = FlippedView::new(mtm);
    let view: Retained<NSView> = Retained::into_super(view);

    // Layer-back the view so we can attach a CAShapeLayer.
    let _: () = unsafe { msg_send![&view, setWantsLayer: true] };

    // Parse every path string in `data.paths` into a single
    // CGMutablePath. Walks via the shared parser in apple/core so
    // iOS and macOS render identical icons from identical input.
    let mut emitter = CGPathEmitter::new();
    for path_d in data.paths {
        parse_svg_path(path_d, sx, sy, &mut emitter);
    }
    let raw_path = emitter.into_raw();

    // Build the CAShapeLayer. `+[CAShapeLayer layer]` returns an
    // autoreleased instance; `Retained::from_raw` would over-
    // release. Use the class method via `msg_send_id` which
    // handles the +0 → +1 transfer.
    let shape_layer: Retained<NSObject> = unsafe {
        let cls = objc2::class!(CAShapeLayer);
        msg_send_id![cls, layer]
    };
    // Render the vector path at the SCREEN scale. A manually-created sublayer
    // does NOT inherit `contentsScale` from the view's backing layer — it
    // defaults to 1.0, so on a Retina display the path is rasterized at 1× and
    // scaled up, making the curves blocky/"boxy" (web's SVG stays vector-crisp).
    let screen_scale: f64 = unsafe {
        let screen: *mut NSObject = msg_send![objc2::class!(NSScreen), mainScreen];
        if screen.is_null() {
            2.0
        } else {
            msg_send![screen, backingScaleFactor]
        }
    };
    let _: () = unsafe { msg_send![&shape_layer, setContentsScale: screen_scale] };

    // setPath: retains the CGPathRef; we own one +1 reference from
    // CGPathCreateMutable that we need to release after the layer
    // takes its own retain.
    // `raw_path` is `*mut CGPathRef`, which now encodes as `^{CGPath=}`.
    let _: () = unsafe { msg_send![&shape_layer, setPath: raw_path] };
    unsafe { CGPathRelease(raw_path) };

    // Icon color via NSColor → CGColor. Matches the iOS path's
    // UIColor → CGColor route. `-[NSColor CGColor]` returns `^{CGColor=}`, so
    // receive it as the encoding-correct `CGColorRef` newtype (a `^v` receiver
    // SIGABRTs) — `setFill/StrokeColor:` take the same typed pointer.
    let ns_color: Retained<NSColor> = match color {
        Some(c) => color_to_nscolor(c),
        None => unsafe { msg_send_id![objc2::class!(NSColor), labelColor] },
    };
    let cg_color: super::CGColorRef = unsafe { msg_send![&ns_color, CGColor] };
    let clear: Retained<NSColor> = unsafe {
        msg_send_id![objc2::class!(NSColor), clearColor]
    };
    let cg_clear: super::CGColorRef = unsafe { msg_send![&clear, CGColor] };

    if data.filled {
        // Filled / silhouette style: fill with the icon color, no stroke.
        let _: () = unsafe { msg_send![&shape_layer, setFillColor: cg_color] };
        let _: () = unsafe { msg_send![&shape_layer, setStrokeColor: cg_clear] };
    } else {
        // Stroke-only (outlined, matches Lucide / iOS posture).
        let _: () = unsafe { msg_send![&shape_layer, setStrokeColor: cg_color] };
        let _: () = unsafe { msg_send![&shape_layer, setFillColor: cg_clear] };
    }

    // Stroke width scaled to target size.
    let line_width: f64 = 2.0 * sx;
    let _: () = unsafe { msg_send![&shape_layer, setLineWidth: line_width] };

    // Rounded caps + joins.
    let round = NSString::from_str("round");
    let _: () = unsafe { msg_send![&shape_layer, setLineCap: &*round] };
    let _: () = unsafe { msg_send![&shape_layer, setLineJoin: &*round] };

    // Fill rule (relevant when `filled`; harmless for stroke-only).
    if data.fill_rule == FillRule::EvenOdd {
        let rule = NSString::from_str("even-odd");
        let _: () = unsafe { msg_send![&shape_layer, setFillRule: &*rule] };
    }

    // Frame the shape layer inside the view's bounds. The path is baked
    // at a fixed 24×24; the layout pass (`sync_icon_sublayer`) scales +
    // centers this layer to the view's actual box, so `Icon(size = N)`
    // renders at N px instead of always 24.
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: SIZE, height: SIZE },
    };
    let _: () = unsafe { msg_send![&shape_layer, setFrame: frame] };

    // Name the sublayer so the layout pass can find + scale it (and only
    // it). Mirrors the gradient sublayer's `idealyst_gradient` marker.
    let marker = NSString::from_str("idealyst_icon");
    let _: () = unsafe { msg_send![&shape_layer, setName: &*marker] };

    // Attach the shape layer to the view's CALayer.
    let layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
    let _: () = unsafe { msg_send![&layer, addSublayer: &*shape_layer] };

    view
}

/// The natural box the icon path is baked into (see `create_icon`'s
/// `SIZE`). The layout-pass scale divides the laid-out box by this.
const ICON_NATURAL: f64 = 24.0;

/// Uniform scale that fits the 24×24 baked glyph into a `width`×`height`
/// box (square: the smaller side wins). Pure so it's unit-testable
/// without a live layer.
fn icon_scale(width: f64, height: f64) -> f64 {
    width.min(height) / ICON_NATURAL
}

/// Scale + center the icon's shape sublayer to the view's laid-out box.
/// Called from the layout (frame) pass on every view — a no-op unless the
/// view owns an `idealyst_icon` sublayer — so it stays cheap for non-icons.
///
/// The path is baked at a fixed `ICON_NATURAL`×`ICON_NATURAL`; this applies
/// a uniform layer transform of `min(w,h) / ICON_NATURAL` about the layer
/// center so `Icon(size = N)` actually renders at N px. Without it the
/// glyph always painted at 24 px regardless of the requested size — the
/// "chevrons are huge" bug. The view's frame (origin + size) is written
/// elsewhere in the same pass; this only touches the glyph sublayer.
pub(crate) fn sync_icon_sublayer(view: &NSView, width: f64, height: f64) {
    // Raw pointers + null checks (NOT `msg_send_id!`, which panics on NULL):
    // this runs on EVERY view in the layout pass, and a non-layer-backed
    // `NSView` returns a nil `layer`. Mirrors `gradient::sync_gradient_sublayer`.
    let layer_ptr: *mut NSObject = unsafe { msg_send![view, layer] };
    if layer_ptr.is_null() {
        return;
    }
    let sublayers_ptr: *mut NSObject = unsafe { msg_send![layer_ptr, sublayers] };
    if sublayers_ptr.is_null() {
        return;
    }
    let count: usize = unsafe { msg_send![sublayers_ptr, count] };
    for i in 0..count {
        let sub_ptr: *mut NSObject = unsafe { msg_send![sublayers_ptr, objectAtIndex: i] };
        if sub_ptr.is_null() {
            continue;
        }
        let name_ptr: *mut NSString = unsafe { msg_send![sub_ptr, name] };
        if name_ptr.is_null() {
            continue;
        }
        if unsafe { &*name_ptr }.to_string() != "idealyst_icon" {
            continue;
        }
        if width.min(height) <= 0.0 {
            return;
        }
        let sub = unsafe { &*sub_ptr };
        let scale = icon_scale(width, height);
        // Skip when the scale already matches — the layer's position +
        // bounds depend only on the (constant) box size, so a matching
        // scale means it's already correct. This makes a normal re-render
        // (color change, label re-render) touch the layer zero times.
        if (super::animated::current_layer_scale(sub) - scale).abs() < 1e-3 {
            return;
        }
        // Disable Core Animation's implicit action so the resize is
        // INSTANT, not a ~0.25s animation. Setting `transform`/`bounds`/
        // `position` on a sublayer inside the run loop otherwise animates —
        // the "icon resizes each update" artifact. The path itself is
        // rasterized once by `CAShapeLayer` (CA caches the bitmap); the
        // transform is GPU compositing, not a re-rasterization.
        unsafe {
            let _: () = msg_send![class!(CATransaction), begin];
            let _: () = msg_send![class!(CATransaction), setDisableActions: true];
            // Keep the 24×24 path space; center it in the view and scale.
            let _: () = msg_send![sub, setAnchorPoint: CGPoint { x: 0.5, y: 0.5 }];
            let _: () = msg_send![
                sub,
                setBounds: CGRect {
                    origin: CGPoint { x: 0.0, y: 0.0 },
                    size: CGSize { width: ICON_NATURAL, height: ICON_NATURAL },
                }
            ];
            let _: () = msg_send![
                sub,
                setPosition: CGPoint { x: width / 2.0, y: height / 2.0 }
            ];
        }
        super::animated::apply_layer_scale(sub, scale);
        unsafe {
            let _: () = msg_send![class!(CATransaction), commit];
        }
        return;
    }
}

/// Update the color on an existing icon view's CAShapeLayer. Walks the
/// layer's first sublayer (which we know is the shape layer because
/// `create_icon` adds exactly one) and rewrites whichever paint is
/// active: a filled icon has a non-clear `fillColor`, an outlined icon a
/// clear fill.
pub(crate) fn update_icon_color(node: &MacosNode, color: &Color) {
    if let Some(shape) = get_shape_layer(node) {
        let ns = color_to_nscolor(color);
        let cg: super::CGColorRef = unsafe { msg_send![&ns, CGColor] };
        let cg_fill: super::CGColorRef = unsafe { msg_send![&shape, fillColor] };
        let is_filled = !cg_fill.0.is_null() && unsafe { CGColorGetAlpha(cg_fill.0) } > 0.0;
        if is_filled {
            let _: () = unsafe { msg_send![&shape, setFillColor: cg] };
        } else {
            let _: () = unsafe { msg_send![&shape, setStrokeColor: cg] };
        }
    }
}

fn get_shape_layer(node: &MacosNode) -> Option<Retained<NSObject>> {
    let view = node.as_view();
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    let sublayers: *const NSObject = unsafe { msg_send![&layer, sublayers] };
    if sublayers.is_null() {
        return None;
    }
    let count: usize = unsafe { msg_send![sublayers, count] };
    if count == 0 {
        return None;
    }
    let shape: Retained<NSObject> = unsafe { msg_send_id![sublayers, firstObject] };
    Some(shape)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression for the `-[CAShapeLayer setPath:]` SIGABRT: the runtime wants
    /// the argument typed `^{CGPath=}`, and a `*mut CGPathRef` only carries that
    /// encoding because `CGPathRef: RefEncode` says so. If someone reverts the
    /// pointer to a bare `*const c_void` (`^v`) the icon backend crashes the
    /// instant any `icon()` mounts on macOS (the whiteboard-demo did exactly
    /// this). A full mount test needs a live AppKit layer + main thread; pinning
    /// the encoding constant is the closest reachable guard.
    /// `Icon(size = N)` must render at N px, not the baked 24. The objc
    /// layer transform needs a live main-thread layer, so the closest
    /// reachable guard is the pure scale ratio the layout pass applies:
    /// a 13 px box → 13/24, a 24 px box → identity, a 48 px box → 2×.
    #[test]
    fn icon_scale_fits_box_to_24px_glyph() {
        assert!((icon_scale(24.0, 24.0) - 1.0).abs() < 1e-9, "24px box = identity");
        assert!((icon_scale(13.0, 13.0) - 13.0 / 24.0).abs() < 1e-9, "13px box scales down");
        assert!((icon_scale(48.0, 48.0) - 2.0).abs() < 1e-9, "48px box scales up");
        // Non-square: the smaller side wins (square glyph).
        assert!((icon_scale(40.0, 12.0) - 0.5).abs() < 1e-9, "min side drives scale");
    }

    #[test]
    fn cgpath_pointer_encodes_as_typed_cgpath() {
        assert_eq!(
            CGPathRef::ENCODING_REF,
            Encoding::Pointer(&Encoding::Struct("CGPath", &[])),
            "*mut CGPathRef must encode as ^{{CGPath=}} for -[CAShapeLayer setPath:]",
        );
    }
}
