//! Native screen capture for the UIKit backend — the on-device side of
//! the Robot bridge's `"screenshot"` verb.
//!
//! `-[UIView drawViewHierarchyInRect:afterScreenUpdates:]` rasterizes the
//! live view hierarchy (including most GPU-backed `CAMetalLayer` content)
//! into the current image context. We snapshot the host root view — the
//! surface the framework renders into — at device-native scale.
//!
//! ## Why the deprecated `UIGraphics*` context API
//!
//! `UIGraphicsImageRenderer` (iOS 10+) is the modern path but takes an
//! ObjC block, which is awkward to bridge from Rust. The older
//! `UIGraphicsBeginImageContextWithOptions` family is block-free, still
//! shipping in current SDKs, and produces an identical result for a
//! one-shot capture — the right trade-off for a debug utility. UIKit is
//! linked by the host app's link step (the same arrangement the rest of
//! this backend's ObjC calls rely on), so these C symbols resolve there.

use objc2::msg_send;
use objc2::runtime::AnyObject;
use objc2_foundation::{CGFloat, CGRect, CGSize};
use objc2_ui_kit::UIView;
use runtime_core::Screenshot;

extern "C" {
    fn UIGraphicsBeginImageContextWithOptions(size: CGSize, opaque: bool, scale: CGFloat);
    fn UIGraphicsGetImageFromCurrentImageContext() -> *mut AnyObject; // UIImage*
    fn UIGraphicsEndImageContext();
    fn UIImagePNGRepresentation(image: *mut AnyObject) -> *mut AnyObject; // NSData*
}

/// Capture `view` (the host root) as a PNG. Must run on the main thread —
/// the Robot bridge polls there, so the caller already satisfies it.
pub(crate) fn capture(view: &UIView) -> Result<Screenshot, String> {
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    if bounds.size.width <= 0.0 || bounds.size.height <= 0.0 {
        return Err("host root has zero bounds (not laid out yet)".into());
    }

    // scale 0.0 → device native scale (Retina-correct); opaque=false keeps
    // the alpha channel so transparent regions don't render as black.
    unsafe { UIGraphicsBeginImageContextWithOptions(bounds.size, false, 0.0) };
    // afterScreenUpdates:true forces a fresh commit so the snapshot
    // reflects the current frame rather than a stale presentation.
    let drawn: bool =
        unsafe { msg_send![view, drawViewHierarchyInRect: bounds, afterScreenUpdates: true] };
    let image: *mut AnyObject = unsafe { UIGraphicsGetImageFromCurrentImageContext() };
    unsafe { UIGraphicsEndImageContext() };

    if !drawn || image.is_null() {
        return Err("drawViewHierarchyInRect failed".into());
    }

    let data: *mut AnyObject = unsafe { UIImagePNGRepresentation(image) };
    if data.is_null() {
        return Err("UIImagePNGRepresentation returned nil".into());
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

    // Pixel dimensions = point size × the UIImage's backing scale, so the
    // reported size matches the encoded PNG.
    let scale: CGFloat = unsafe { msg_send![image, scale] };
    let width = (bounds.size.width * scale).round().max(0.0) as u32;
    let height = (bounds.size.height * scale).round().max(0.0) as u32;

    Ok(Screenshot { png, width, height })
}
