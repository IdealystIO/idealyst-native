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
//! Mirror iOS's `create_icon`: stroke-only with rounded caps, line
//! width = 2 × scale factor. Authors can switch to fill later
//! through a future style attribute; current `IconData` doesn't
//! carry a stroke/fill discriminant.

use std::ffi::c_void;

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

    // setPath: retains the CGPathRef; we own one +1 reference from
    // CGPathCreateMutable that we need to release after the layer
    // takes its own retain.
    let _: () = unsafe { msg_send![&shape_layer, setPath: raw_path as *const c_void] };
    unsafe { CGPathRelease(raw_path) };

    // Stroke color via NSColor → CGColor. Matches the iOS path's
    // UIColor → CGColor route.
    let ns_color: Retained<NSColor> = match color {
        Some(c) => color_to_nscolor(c),
        None => unsafe { msg_send_id![objc2::class!(NSColor), labelColor] },
    };
    let cg_color: *const c_void = unsafe { msg_send![&ns_color, CGColor] };
    let _: () = unsafe { msg_send![&shape_layer, setStrokeColor: cg_color] };

    // No fill — outlined icons (matches Lucide / iOS posture).
    let clear: Retained<NSColor> = unsafe {
        msg_send_id![objc2::class!(NSColor), clearColor]
    };
    let cg_clear: *const c_void = unsafe { msg_send![&clear, CGColor] };
    let _: () = unsafe { msg_send![&shape_layer, setFillColor: cg_clear] };

    // Stroke width scaled to target size.
    let line_width: f64 = 2.0 * sx;
    let _: () = unsafe { msg_send![&shape_layer, setLineWidth: line_width] };

    // Rounded caps + joins.
    let round = NSString::from_str("round");
    let _: () = unsafe { msg_send![&shape_layer, setLineCap: &*round] };
    let _: () = unsafe { msg_send![&shape_layer, setLineJoin: &*round] };

    // Fill rule (only relevant if a future style attribute swaps
    // to fill-only; harmless for stroke-only).
    if data.fill_rule == FillRule::EvenOdd {
        let rule = NSString::from_str("even-odd");
        let _: () = unsafe { msg_send![&shape_layer, setFillRule: &*rule] };
    }

    // Frame the shape layer inside the view's bounds.
    let frame = CGRect {
        origin: CGPoint { x: 0.0, y: 0.0 },
        size: CGSize { width: SIZE, height: SIZE },
    };
    let _: () = unsafe { msg_send![&shape_layer, setFrame: frame] };

    // Attach the shape layer to the view's CALayer.
    let layer: Retained<NSObject> = unsafe { msg_send_id![&view, layer] };
    let _: () = unsafe { msg_send![&layer, addSublayer: &*shape_layer] };

    view
}

/// Update the stroke color on an existing icon view's CAShapeLayer.
/// Walks the layer's first sublayer (which we know is the shape
/// layer because `create_icon` adds exactly one).
pub(crate) fn update_icon_color(node: &MacosNode, color: &Color) {
    if let Some(shape) = get_shape_layer(node) {
        let ns = color_to_nscolor(color);
        let cg: *const c_void = unsafe { msg_send![&ns, CGColor] };
        let _: () = unsafe { msg_send![&shape, setStrokeColor: cg] };
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
