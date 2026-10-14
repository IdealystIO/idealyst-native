//! Apple platform wrappers for image-source loading, shared by the iOS and
//! macOS image views: the CoreGraphics `CGImage` an SVG raster is handed to
//! `UIImage`/`NSImage` through, `nsdata_bytes` for reading fetched / asset
//! `NSData`, the `NSURLSession` fetch with its main-thread hop
//! ([`fetch_image`](crate::image_source::fetch_image)), and the app-bundle lookup for `asset!` files
//! ([`read_bundled_asset`](crate::image_source::read_bundled_asset)). The decisions they carry out (what a `src`
//! means, what a fetch result means, which load is current) are
//! [`image_load_policy`](crate::image_load_policy).
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


/// A fetched image body, ready for the main thread.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub enum Fetched {
    /// Decoded by the platform's bitmap decoder (`UIImage` / `NSImage`).
    Bitmap(objc2::rc::Retained<objc2_foundation::NSObject>),
    /// SVG bytes (an `NSData`). Parsed on the main thread because the view
    /// keeps the document to re-rasterize when it resizes.
    Svg(objc2::rc::Retained<objc2::runtime::AnyObject>),
}

/// Fetch `url` with `NSURLSession.sharedSession` and call `done` **on the
/// main thread** with the result. Shared by the iOS and macOS image views;
/// each passes its own `decode_bitmap` (`UIImage` / `NSImage` from an
/// `NSData`), which runs on the session's background queue so decoding
/// stays off the main thread.
///
/// Every outcome reaches `done`, following `docs/image-src.md`: a URL that
/// `NSURL` can't parse (e.g. a host with a space) or a task the session
/// won't create calls `done(Err(UnstartableFetch))` right away. These
/// returned silently before, leaving a blank view with no `on_error`. A
/// transport error, a non-2xx status or an empty body is an `Err` from
/// [`fetch_verdict`](crate::image_load_policy::fetch_verdict), as are bytes
/// the decoder rejects.
///
/// `done` is not `Send` (it holds the view); it only travels to the main
/// queue and runs there. The caller checks its own load ticket inside
/// `done`. This function does not know which load is current.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub fn fetch_image(
    url: &str,
    decode_bitmap: fn(&objc2::runtime::AnyObject) -> Option<objc2::rc::Retained<objc2_foundation::NSObject>>,
    done: impl FnOnce(Result<Fetched, crate::image_load_policy::SrcFailure>) + 'static,
) {
    use crate::image_load_policy::{fetch_verdict, FetchError, FetchResult, FetchVerdict, SrcFailure};
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{msg_send, msg_send_id};
    use std::sync::Mutex;

    let ns_url_str = objc2_foundation::NSString::from_str(url);
    let ns_url: Option<Retained<AnyObject>> =
        unsafe { msg_send_id![objc2::class!(NSURL), URLWithString: &*ns_url_str] };
    let Some(ns_url) = ns_url else {
        done(Err(SrcFailure::UnstartableFetch));
        return;
    };
    let session: *mut AnyObject = unsafe { msg_send![objc2::class!(NSURLSession), sharedSession] };
    if session.is_null() {
        done(Err(SrcFailure::UnstartableFetch));
        return;
    }

    // The block is `Fn` and may in principle be dropped without running;
    // `done` sits in a slot so it is taken exactly once. If the task can't
    // be created, the slot hands `done` back for the synchronous error.
    type Done = Box<dyn FnOnce(Result<Fetched, SrcFailure>)>;
    let slot: std::sync::Arc<Mutex<Option<Done>>> = std::sync::Arc::new(Mutex::new(Some(Box::new(done))));
    let slot_in_block = slot.clone();
    let completion = block2::RcBlock::new(
        move |data: *mut AnyObject, response: *mut AnyObject, error: *mut AnyObject| {
            // Runs on the session's background queue. Crash-loud on panic:
            // an unwind through this `extern "C"` boundary would abort with
            // no message.
            crate::crash::abort_on_panic("image URL completion handler", || {
                let Some(done) = slot_in_block.lock().ok().and_then(|mut s| s.take()) else {
                    return;
                };
                let body = (!data.is_null()).then(|| unsafe { nsdata_bytes(&*data) });
                let verdict = fetch_verdict(&FetchResult {
                    transport_error: !error.is_null(),
                    http_status: unsafe { http_status(response) },
                    body,
                });
                let result = match verdict {
                    FetchVerdict::Error(e) => Err(SrcFailure::Fetch(e)),
                    FetchVerdict::Svg => unsafe { Retained::retain(data) }
                        .map(Fetched::Svg)
                        .ok_or(SrcFailure::Fetch(FetchError::EmptyBody)),
                    FetchVerdict::Bitmap => decode_bitmap(unsafe { &*data })
                        .map(Fetched::Bitmap)
                        .ok_or(SrcFailure::UndecodableBitmap),
                };
                unsafe { run_on_main(move || done(result)) };
            });
        },
    );
    let task: *mut AnyObject = unsafe {
        msg_send![session, dataTaskWithURL: &*ns_url, completionHandler: &*completion]
    };
    if task.is_null() {
        let done = slot.lock().ok().and_then(|mut s| s.take());
        if let Some(done) = done {
            done(Err(SrcFailure::UnstartableFetch));
        }
        return;
    }
    let _: () = unsafe { msg_send![task, resume] };
}

/// `NSHTTPURLResponse.statusCode`, or `None` for a nil / non-HTTP response.
///
/// # Safety
/// `response` is nil or a valid `NSURLResponse`.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
unsafe fn http_status(response: *mut objc2::runtime::AnyObject) -> Option<i64> {
    if response.is_null() {
        return None;
    }
    let is_http: bool = unsafe {
        objc2::msg_send![response, isKindOfClass: objc2::class!(NSHTTPURLResponse)]
    };
    is_http.then(|| {
        let code: isize = unsafe { objc2::msg_send![response, statusCode] };
        code as i64
    })
}

/// Run `f` on the main queue; callable from any thread.
///
/// # Safety
/// `f` is not required to be `Send` because its captures are Objective-C
/// objects (atomically refcounted, safe to move between threads) that it
/// only *uses* on the main thread. The caller must not capture `Rc`s or
/// other thread-unsafe Rust state that another thread can still reach.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub unsafe fn run_on_main(f: impl FnOnce() + 'static) {
    use std::sync::Mutex;
    let slot: Mutex<Option<Box<dyn FnOnce()>>> = Mutex::new(Some(Box::new(f)));
    let block = block2::RcBlock::new(move || {
        crate::crash::abort_on_panic("image load main-queue hop", || {
            let f = slot.lock().ok().and_then(|mut s| s.take());
            if let Some(f) = f {
                f();
            }
        });
    });
    // `dispatch_async` copies (retains) the block itself, so our `RcBlock`
    // drops normally once the call returns.
    unsafe {
        crate::scheduler::dispatch_async(
            &crate::scheduler::_dispatch_main_q as *const _ as *const std::ffi::c_void,
            &*block as *const _ as *const std::ffi::c_void,
        );
    }
}

/// `NSBundle.mainBundle.resourcePath`: `<App>.app` on iOS,
/// `<App>.app/Contents/Resources` on macOS, the executable's directory for
/// an unbundled binary (`cargo test`).
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub fn bundle_resource_dir() -> Option<std::path::PathBuf> {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2_foundation::NSString;
    let bundle: Option<Retained<AnyObject>> =
        unsafe { objc2::msg_send_id![objc2::class!(NSBundle), mainBundle] };
    let path: Option<Retained<NSString>> =
        unsafe { objc2::msg_send_id![&*bundle?, resourcePath] };
    Some(std::path::PathBuf::from(path?.to_string()))
}

/// Read an `asset!` file from the app bundle: `<resources>/assets/<path>`
/// (see [`bundled_asset_path`](crate::image_load_policy::bundled_asset_path)).
/// `None` when the path is unsafe or the build didn't ship the file.
#[cfg(any(target_os = "ios", target_os = "tvos", target_os = "macos"))]
pub fn read_bundled_asset(path: &str) -> Option<Vec<u8>> {
    let file = crate::image_load_policy::bundled_asset_path(&bundle_resource_dir()?, path)?;
    std::fs::read(file).ok()
}
