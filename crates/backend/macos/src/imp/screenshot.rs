//! Native screen capture for the AppKit backend — the on-device side of
//! the Robot bridge's `"screenshot"` verb.
//!
//! The host root's layer trees are rasterized with
//! `-[CALayer renderInContext:]` into a bitmap at the window's backing
//! scale, so the PNG is Retina-correct. We snapshot the host root view —
//! the surface the framework renders into — which is what an author
//! thinks of as "the app screen".
//!
//! ## Why the layer trees, not the window image or `cacheDisplayInRect:`
//!
//! `CGWindowListCreateImage` / ScreenCaptureKit would grab the composited
//! window, but they need Screen Recording permission (a system prompt).
//! Rendering in-process is permission-free and captures exactly the app
//! content. `cacheDisplayInRect:` is in-process too, but it walks the VIEW
//! tree in `subviews` order, so it paints a `z_index`-raised sibling under
//! the later sibling it covers on screen and cannot stack a reactive
//! anchor's children among the anchor's siblings. Rendering the layer
//! trees with each put in Core Animation's paint order for the duration
//! (`backend_apple_core::layer_paint_order`) matches the compositor; that
//! module's live test checks both against `CARenderer`. Content rendered
//! on a *separate* surface (a `Graphics` primitive's `CAMetalLayer`, live
//! video) is not in the layer's model contents, so it is missing from the
//! shot — by `cacheDisplayInRect:` as much as here. Documented, not worked
//! around.

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{msg_send, msg_send_id};
use objc2_app_kit::NSView;
use backend_apple_core::cg::CGImageRef;
use objc2_foundation::{CGFloat, CGRect};
use runtime_shared::Screenshot;

/// `NSBitmapImageFileTypePNG` — the raw enum value for PNG output. Hard
/// -coded to avoid pulling the `NSBitmapImageRep` typed bindings just for
/// one constant (matches the `class!()`-based style used elsewhere in
/// this backend).
const NS_BITMAP_FILE_TYPE_PNG: usize = 4;

/// The layer-tree roots under `view`, each with the view it backs: its own
/// layer when it is layer-backed (every descendant then is too), else those
/// of its subviews.
fn layer_tree_roots(view: &NSView) -> Vec<(Retained<NSView>, Retained<NSObject>)> {
    let layer: Option<Retained<NSObject>> = unsafe { msg_send_id![view, layer] };
    if let Some(layer) = layer {
        let v = unsafe { Retained::retain(view as *const NSView as *mut NSView) }
            .expect("non-null view");
        return vec![(v, layer)];
    }
    let subviews: Retained<objc2_foundation::NSArray<NSView>> =
        unsafe { msg_send_id![view, subviews] };
    subviews.iter().flat_map(|sub| layer_tree_roots(sub)).collect()
}

/// `CGContextRef`, encoded as `^{CGContext=}` for `renderInContext:`.
#[repr(transparent)]
#[derive(Clone, Copy)]
struct CGContextRef(*mut std::ffi::c_void);
unsafe impl objc2::encode::Encode for CGContextRef {
    const ENCODING: objc2::encode::Encoding =
        objc2::encode::Encoding::Pointer(&objc2::encode::Encoding::Struct("CGContext", &[]));
}

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    static kCGColorSpaceSRGB: *const std::ffi::c_void;
    fn CGColorSpaceCreateWithName(name: *const std::ffi::c_void) -> *mut std::ffi::c_void;
    fn CGColorSpaceRelease(cs: *mut std::ffi::c_void);
    fn CGBitmapContextCreate(
        data: *mut std::ffi::c_void,
        width: usize,
        height: usize,
        bits_per_component: usize,
        bytes_per_row: usize,
        space: *mut std::ffi::c_void,
        bitmap_info: u32,
    ) -> CGContextRef;
    fn CGBitmapContextCreateImage(ctx: CGContextRef) -> CGImageRef;
    fn CGContextRelease(ctx: CGContextRef);
    fn CGImageRelease(image: CGImageRef);
    fn CGContextScaleCTM(ctx: CGContextRef, sx: CGFloat, sy: CGFloat);
    fn CGContextTranslateCTM(ctx: CGContextRef, tx: CGFloat, ty: CGFloat);
    fn CGContextSaveGState(ctx: CGContextRef);
    fn CGContextRestoreGState(ctx: CGContextRef);
}

/// `kCGImageAlphaPremultipliedLast` — RGBA, premultiplied.
const ALPHA_PREMULTIPLIED_LAST: u32 = 1;

/// Capture `view` (the host root) as a PNG. Must run on the main thread —
/// the Robot bridge polls there, so the caller already satisfies it.
pub(crate) fn capture(view: &NSView) -> Result<Screenshot, String> {
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return Err("host root has zero bounds (not laid out yet)".into());
    }
    let window: *mut AnyObject = unsafe { msg_send![view, window] };
    let scale: CGFloat =
        if window.is_null() { 1.0 } else { unsafe { msg_send![window, backingScaleFactor] } };
    let pw = (bounds.size.width * scale).round() as usize;
    let ph = (bounds.size.height * scale).round() as usize;

    // Bring the layer tree up to date first: `drawRect:`-drawn views
    // (labels) paint into their layer during AppKit's display pass, and
    // AppKit attaches a newly layer-backed subview's layer to its parent's
    // only when the pending Core Animation transaction commits — before
    // the flush, a subtree mounted this runloop turn has no sublayers and
    // `renderInContext:` draws nothing for it.
    let _: () = unsafe { msg_send![view, displayIfNeeded] };
    let _: () = unsafe { msg_send![objc2::class!(CATransaction), flush] };

    let cs = unsafe { CGColorSpaceCreateWithName(kCGColorSpaceSRGB) };
    let ctx = unsafe {
        CGBitmapContextCreate(std::ptr::null_mut(), pw, ph, 8, pw * 4, cs, ALPHA_PREMULTIPLIED_LAST)
    };
    unsafe { CGColorSpaceRelease(cs) };
    if ctx.0.is_null() {
        return Err("CGBitmapContextCreate failed".into());
    }
    unsafe { CGContextScaleCTM(ctx, scale, scale) };

    // Rasterize the layer trees ourselves rather than through
    // `cacheDisplayInRect:`. That walks the VIEW tree in `subviews` order
    // (AppKit even re-syncs each layer's `sublayers` to it mid-capture), so
    // it can honor neither the `zPosition` that `z_index` paints through
    // nor a reactive anchor's `CATransformLayer`, whose children stack
    // against the anchor's siblings — the shot showed a raised sibling
    // UNDER the one it covers on screen. `renderInContext:` on each layer
    // tree, with the trees put in compositor paint order for the duration
    // (`backend_apple_core::layer_paint_order`), matches the screen.
    let flipped: bool = unsafe { msg_send![view, isFlipped] };
    let roots = layer_tree_roots(view);
    let layers: Vec<&NSObject> = roots.iter().map(|(_, l)| &**l).collect();
    backend_apple_core::layer_paint_order::with_layer_trees_in_paint_order(&layers, || {
        for (v, layer) in &roots {
            let vb: CGRect = unsafe { msg_send![&**v, bounds] };
            let r: CGRect = unsafe { msg_send![&**v, convertRect: vb, toView: view] };
            // The bitmap context is y-up from the host's bottom-left.
            let x = r.origin.x - bounds.origin.x;
            let y = if flipped {
                bounds.size.height - (r.origin.y - bounds.origin.y) - r.size.height
            } else {
                r.origin.y - bounds.origin.y
            };
            unsafe {
                CGContextSaveGState(ctx);
                CGContextTranslateCTM(ctx, x, y);
                let _: () = msg_send![&**layer, renderInContext: ctx];
                CGContextRestoreGState(ctx);
            }
        }
    });

    let image = unsafe { CGBitmapContextCreateImage(ctx) };
    unsafe { CGContextRelease(ctx) };
    if image.0.is_null() {
        return Err("CGBitmapContextCreateImage failed".into());
    }
    let rep: Option<Retained<AnyObject>> = unsafe {
        let alloc: *mut AnyObject = msg_send![objc2::class!(NSBitmapImageRep), alloc];
        let rep: *mut AnyObject = msg_send![alloc, initWithCGImage: image];
        Retained::from_raw(rep)
    };
    unsafe { CGImageRelease(image) };
    let Some(rep) = rep else {
        return Err("NSBitmapImageRep initWithCGImage: returned nil".into());
    };
    let rep: &AnyObject = &rep;

    // Encode the rep as PNG NSData. An empty properties dictionary uses
    // AppKit's defaults (no interlacing / gamma tweaks).
    let empty_props: *mut AnyObject =
        unsafe { msg_send![objc2::class!(NSDictionary), dictionary] };
    let data: *mut AnyObject = unsafe {
        msg_send![rep, representationUsingType: NS_BITMAP_FILE_TYPE_PNG, properties: empty_props]
    };
    if data.is_null() {
        return Err("representationUsingType: returned nil PNG data".into());
    }

    let len: usize = unsafe { msg_send![data, length] };
    // `-[NSData bytes]` returns `const void *` (objc encoding `^v`), so the
    // receiver type must be `*const c_void` — declaring `*const u8` (`*`)
    // trips objc2's return-encoding check at runtime. Cast after.
    let bytes_ptr: *const core::ffi::c_void = unsafe { msg_send![data, bytes] };
    let bytes_ptr = bytes_ptr as *const u8;
    if bytes_ptr.is_null() || len == 0 {
        return Err("captured PNG data was empty".into());
    }
    // Copy out of the autoreleased NSData before it's reclaimed.
    let png = unsafe { std::slice::from_raw_parts(bytes_ptr, len) }.to_vec();

    // Report the bitmap's pixel dimensions (backing-scaled) so the size
    // matches the PNG, not the point-space `bounds`.
    let width: isize = unsafe { msg_send![rep, pixelsWide] };
    let height: isize = unsafe { msg_send![rep, pixelsHigh] };

    Ok(Screenshot {
        png,
        width: width.max(0) as u32,
        height: height.max(0) as u32,
    })
}
