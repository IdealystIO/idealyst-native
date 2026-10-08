//! Apple platform wrappers for image-source decoding: the CoreGraphics
//! `CGImage` an SVG raster is handed to `UIImage`/`NSImage` through, and
//! `nsdata_bytes` for reading fetched / asset `NSData`.
//!
//! The decoding itself (`data:` URIs, SVG sniffing, resvg rasterization) is
//! platform-neutral and lives in the `backend-image-source` crate, shared
//! with Android — see that crate for why it is a crate of its own. Only the
//! CoreGraphics / Foundation glue stays here, so the module is empty on a
//! non-Apple host.

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
use backend_image_source::SvgRaster;

/// Owned `CGImageRef` (+1). Released on drop.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub struct OwnedCGImage(*mut std::ffi::c_void);

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
impl OwnedCGImage {
    /// The `CGImageRef` (typed so objc2's encoding check passes), borrowed
    /// for the lifetime of `self`. Pass it to
    /// `+[UIImage imageWithCGImage:…]` / `-[NSImage initWithCGImage:size:]`,
    /// which retain it.
    pub fn as_cg_ref(&self) -> crate::cg::CGImageRef {
        crate::cg::CGImageRef(self.0)
    }
}

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
impl Drop for OwnedCGImage {
    fn drop(&mut self) {
        unsafe { cg_ffi::CGImageRelease(self.0) };
    }
}

#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
mod cg_ffi {
    use std::ffi::c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        pub fn CFDataCreate(alloc: *const c_void, bytes: *const u8, len: isize) -> *mut c_void;
        pub fn CFRelease(cf: *const c_void);
    }
    // Signatures match `font.rs`'s declarations of the same symbols —
    // rustc warns when two `extern` blocks disagree.
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        pub fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
        pub fn CGColorSpaceRelease(cs: *mut c_void);
        pub fn CGDataProviderCreateWithCFData(data: *const c_void) -> *const c_void;
        pub fn CGDataProviderRelease(p: *const c_void);
        #[allow(clippy::too_many_arguments)]
        pub fn CGImageCreate(
            width: usize,
            height: usize,
            bits_per_component: usize,
            bits_per_pixel: usize,
            bytes_per_row: usize,
            space: *mut c_void,
            bitmap_info: u32,
            provider: *const c_void,
            decode: *const f64,
            should_interpolate: bool,
            intent: i32,
        ) -> *mut c_void;
        pub fn CGImageRelease(image: *mut c_void);
    }
    /// `kCGImageAlphaPremultipliedLast` | `kCGBitmapByteOrderDefault`:
    /// R,G,B,A bytes in memory order, color premultiplied — tiny-skia's
    /// pixmap layout.
    pub const PREMULTIPLIED_RGBA: u32 = 1;
    pub const RENDERING_INTENT_DEFAULT: i32 = 0;
}

/// Borrow an `NSData`'s bytes. `-bytes` is declared `const void *`
/// (`r^v`), which objc2 checks in debug builds — read it as `c_void`, then
/// cast.
///
/// # Safety
/// `data` must be a valid `NSData` that outlives (and is not mutated
/// during) the returned borrow.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub unsafe fn nsdata_bytes<'a>(data: &'a objc2::runtime::AnyObject) -> &'a [u8] {
    let len: usize = unsafe { objc2::msg_send![data, length] };
    let ptr: *const std::ffi::c_void = unsafe { objc2::msg_send![data, bytes] };
    if ptr.is_null() || len == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(ptr.cast::<u8>(), len) }
    }
}

/// Wrap a raster in a `CGImage` (copies the pixels into a `CFData`).
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub fn cg_image_from_raster(raster: &SvgRaster) -> Option<OwnedCGImage> {
    use cg_ffi::*;
    unsafe {
        let data = CFDataCreate(
            std::ptr::null(),
            raster.rgba_premultiplied.as_ptr(),
            raster.rgba_premultiplied.len() as isize,
        );
        if data.is_null() {
            return None;
        }
        let provider = CGDataProviderCreateWithCFData(data);
        CFRelease(data);
        if provider.is_null() {
            return None;
        }
        let space = CGColorSpaceCreateDeviceRGB();
        let image = CGImageCreate(
            raster.width_px as usize,
            raster.height_px as usize,
            8,
            32,
            raster.width_px as usize * 4,
            space,
            PREMULTIPLIED_RGBA,
            provider,
            std::ptr::null(),
            true,
            RENDERING_INTENT_DEFAULT,
        );
        CGColorSpaceRelease(space);
        CGDataProviderRelease(provider);
        (!image.is_null()).then(|| OwnedCGImage(image))
    }
}

