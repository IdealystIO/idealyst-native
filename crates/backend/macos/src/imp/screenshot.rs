//! Native screen capture for the AppKit backend — the on-device side of
//! the Robot bridge's `"screenshot"` verb.
//!
//! `-[NSView cacheDisplayInRect:toBitmap:]` rasterizes the layer-backed
//! view hierarchy (every `NSView` plus its `CALayer` content) into an
//! `NSBitmapImageRep` at the view's backing scale, so the PNG is
//! Retina-correct. We snapshot the host root view — the surface the
//! framework renders into — which is what an author thinks of as "the
//! app screen".
//!
//! ## Why the view hierarchy, not the window image
//!
//! `CGWindowListCreateImage` would grab the whole window incl. the
//! titlebar, but it needs Screen Recording permission (a system prompt)
//! and pulls in CoreGraphics window-server plumbing. `cacheDisplayInRect`
//! is permission-free, in-process, and captures exactly the app content —
//! the right trade-off for a debug utility. The known gap is the same one
//! the iOS/Android impls have: content rendered on a *separate* surface
//! (a `Graphics` primitive's `CAMetalLayer`) draws through its layer here
//! because the layer participates in `cacheDisplayInRect`, but live video
//! layers may snapshot a frame behind. Documented, not worked around.

use objc2::msg_send;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::CGRect;
use runtime_core::Screenshot;

/// `NSBitmapImageFileTypePNG` — the raw enum value for PNG output. Hard
/// -coded to avoid pulling the `NSBitmapImageRep` typed bindings just for
/// one constant (matches the `class!()`-based style used elsewhere in
/// this backend).
const NS_BITMAP_FILE_TYPE_PNG: usize = 4;

/// Capture `view` (the host root) as a PNG. Must run on the main thread —
/// the Robot bridge polls there, so the caller already satisfies it.
pub(crate) fn capture(view: &NSView) -> Result<Screenshot, String> {
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return Err("host root has zero bounds (not laid out yet)".into());
    }

    // Allocate a bitmap rep sized to `bounds` at the view's backing scale,
    // then render the hierarchy into it.
    let rep: *mut AnyObject =
        unsafe { msg_send![view, bitmapImageRepForCachingDisplayInRect: bounds] };
    if rep.is_null() {
        return Err("bitmapImageRepForCachingDisplayInRect returned nil".into());
    }
    let _: () = unsafe { msg_send![view, cacheDisplayInRect: bounds, toBitmapImageRep: rep] };

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
