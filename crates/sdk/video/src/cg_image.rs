//! CoreGraphics RGBA8 → `CGImage` bridge, shared by the Apple backends
//! (`ios.rs` + `macos.rs`).
//!
//! Both the iOS (`AVPlayerLayer` host) and macOS (`NSView`) Video impls
//! display a live `MediaStream`'s frames by pushing them to a `CALayer`'s
//! `contents` as a `CGImage`. The conversion from tightly-packed RGBA8 bytes
//! to a `CGImage` is pure CoreGraphics — identical on iOS and macOS — so it
//! lives here once instead of being duplicated per backend. Mirrors the
//! camera SDK's `#[link] extern "C"` posture (CF types as opaque pointers).

#![allow(dead_code)] // each backend uses a subset; both compile this module.

pub type CGImageRef = *mut std::ffi::c_void;
type CGColorSpaceRef = *mut std::ffi::c_void;
type CGDataProviderRef = *mut std::ffi::c_void;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGColorSpaceCreateDeviceRGB() -> CGColorSpaceRef;
    fn CGColorSpaceRelease(cs: CGColorSpaceRef);
    fn CGDataProviderCreateWithData(
        info: *mut std::ffi::c_void,
        data: *const std::ffi::c_void,
        size: usize,
        release: Option<unsafe extern "C" fn(*mut std::ffi::c_void, *const std::ffi::c_void, usize)>,
    ) -> CGDataProviderRef;
    fn CGDataProviderRelease(p: CGDataProviderRef);
    #[allow(clippy::too_many_arguments)]
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: CGColorSpaceRef,
        bitmap_info: u32,
        provider: CGDataProviderRef,
        decode: *const f64,
        should_interpolate: bool,
        intent: u32,
    ) -> CGImageRef;
    pub fn CGImageRelease(img: CGImageRef);
}

// kCGImageAlphaPremultipliedLast. Camera/screen frames are opaque
// (alpha == 255), so premultiplied == straight; bytes are R,G,B,A.
const CG_ALPHA_PREMUL_LAST: u32 = 1;

/// CGDataProvider release callback — frees the `Vec<u8>` the provider owned.
unsafe extern "C" fn release_pixels(
    info: *mut std::ffi::c_void,
    _data: *const std::ffi::c_void,
    _size: usize,
) {
    if !info.is_null() {
        drop(Box::from_raw(info as *mut Vec<u8>));
    }
}

/// Build a CGImage from tightly-packed RGBA8 `pixels`, taking ownership of
/// the buffer — the data provider frees it (via [`release_pixels`]) when the
/// CGImage is released, so the image is independent of any reused scratch.
/// Returns null on bad input.
pub unsafe fn cgimage_from_rgba(pixels: Vec<u8>, width: usize, height: usize) -> CGImageRef {
    if width == 0 || height == 0 || pixels.len() < width * height * 4 {
        return std::ptr::null_mut();
    }
    let boxed = Box::new(pixels);
    let data_ptr = boxed.as_ptr() as *const std::ffi::c_void;
    let size = boxed.len();
    let info = Box::into_raw(boxed) as *mut std::ffi::c_void;
    let provider = CGDataProviderCreateWithData(info, data_ptr, size, Some(release_pixels));
    if provider.is_null() {
        drop(Box::from_raw(info as *mut Vec<u8>));
        return std::ptr::null_mut();
    }
    let color_space = CGColorSpaceCreateDeviceRGB();
    let image = CGImageCreate(
        width,
        height,
        8,
        32,
        width * 4,
        color_space,
        CG_ALPHA_PREMUL_LAST,
        provider,
        std::ptr::null(),
        false,
        0,
    );
    CGColorSpaceRelease(color_space);
    CGDataProviderRelease(provider); // the CGImage retains it
    image
}
