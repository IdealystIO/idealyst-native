//! `Element::Image` — a layer-backed `NSView` subclass whose backing
//! `CALayer.contents` holds the decoded bitmap, scaled by
//! `contentsGravity`.
//!
//! ## Why not `NSImageView`?
//!
//! `NSImageView.imageScaling` has **no aspect-fill (cover) mode** — it can
//! letterbox (`ProportionallyUpOrDown`) or stretch (`AxesIndependently`)
//! but cannot fill-and-crop. To make [`ObjectFit::Cover`] work — the
//! thumbnail/tile "fill + center-crop" pattern the framework needs — the
//! image must render through the layer's `contents` + `contentsGravity`,
//! which supports all three fits uniformly:
//!
//! | [`ObjectFit`]        | `contentsGravity`   |
//! | -------------------- | ------------------- |
//! | `Fill`               | `resize`            |
//! | `Contain` (default)  | `resizeAspect`      |
//! | `Cover`              | `resizeAspectFill`  |
//!
//! `masksToBounds` is pinned on so a `Cover` image crops to its box (and
//! any author corner-radius clips the bitmap too). This mirrors CSS
//! `object-fit`, UIKit `contentMode`, and Android `ScaleType` on the other
//! backends — one author style, uniform output (CLAUDE.md §7).
//!
//! ## Measurement
//!
//! A plain `NSView` has no `intrinsicContentSize`, so [`ImageView`] stores
//! the bitmap's natural size in an ivar and overrides
//! `intrinsicContentSize` to return it — the Taffy `measure_fn` installed
//! by `install_image_measure` reads that exactly as it did the old
//! `NSImageView`, so an intrinsically-sized image still lays out.
//!
//! ## Sources
//!
//! - **Asset source** (`image_asset(LOGO)`): the walker calls
//!   `register_asset` first; we decode the bytes into an `NSImage` and cache
//!   by id. `create_image` looks up the cached image and assigns its
//!   `CGImage` to the layer.
//! - **URL source** (`image("https://...")`): fetched via `NSURLSession`,
//!   decoded, then assigned on the main thread.
//!
//! `NSImage(data:)` natively decodes PNG, JPG, TIFF, BMP, GIF, ICO, and (on
//! macOS 12+) HEIF and WebP. SVG is not supported; rasterize before
//! embedding.

use std::cell::Cell;
use std::collections::HashMap;

use runtime_core::{AssetId, AssetSource, AssetTag, ObjectFit};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_app_kit::NSView;
use objc2_foundation::{CGRect, CGSize, MainThreadMarker, NSObject, NSString};

use super::MacosNode;

/// `NSImage` cache keyed by [`AssetId`]. Filled by `register_asset`
/// (Embedded → `NSImage(data:)`); queried by `create_image` when
/// `src` is an `asset://{id}` sentinel. Held as `NSObject` because
/// `objc2-app-kit`'s `NSImage` re-export comes through the runtime
/// dispatch, not as a typed pointer we own.
pub(crate) type ImageCache = HashMap<AssetId, Retained<NSObject>>;

const ASSET_URL_PREFIX: &str = "asset://";

// --- The `CALayer.contentsGravity` string for each fit -----------------------
// These are the literal values of the `kCAGravity*` constants; setting them
// by string avoids linking the Core Animation symbols.
const GRAVITY_FILL: &str = "resize";
const GRAVITY_CONTAIN: &str = "resizeAspect";
const GRAVITY_COVER: &str = "resizeAspectFill";

fn gravity_for(fit: ObjectFit) -> &'static str {
    match fit {
        ObjectFit::Fill => GRAVITY_FILL,
        ObjectFit::Contain => GRAVITY_CONTAIN,
        ObjectFit::Cover => GRAVITY_COVER,
    }
}

pub struct ImageViewIvars {
    /// The decoded bitmap's natural size, in points. `{-1, -1}`
    /// (matching `NSViewNoIntrinsicMetric`) until an image is assigned,
    /// so an image awaiting an async URL fetch measures as 0×0 rather
    /// than a bogus size — same as the old empty `NSImageView`.
    natural_size: Cell<CGSize>,
}

declare_class!(
    /// Layer-backed image view: renders `layer.contents` (a `CGImage`) via
    /// `contentsGravity`. See the module doc for why this replaces
    /// `NSImageView`.
    pub struct ImageView;

    unsafe impl ClassType for ImageView {
        type Super = NSView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystImageView";
    }

    impl DeclaredClass for ImageView {
        type Ivars = ImageViewIvars;
    }

    unsafe impl ImageView {
        // Report the bitmap's natural size so the Taffy `measure_fn`
        // (`install_image_measure`) can size an intrinsically-sized image.
        // A plain `NSView` returns `{-1, -1}`; this overrides it.
        #[method(intrinsicContentSize)]
        fn intrinsic_content_size(&self) -> CGSize {
            self.ivars().natural_size.get()
        }

        // Main-thread hop target for the async URL loader. The background
        // completion decodes an `NSImage` then
        // `performSelectorOnMainThread:` calls this with it, so the layer
        // mutation happens on main. Declared so the async path can reference
        // it as a selector.
        #[method(setImageFromNSImage:)]
        fn set_image_from_ns_image(&self, image: &NSObject) {
            let view: &NSView = self;
            set_layer_image(view, image);
        }
    }
);

impl ImageView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ImageViewIvars {
            natural_size: Cell::new(CGSize::new(-1.0, -1.0)),
        });
        let this: Retained<Self> = unsafe { msg_send_id![super(this), init] };
        // Layer-backed so `contents`/`contentsGravity` render; clip so a
        // `Cover` bitmap crops to the box and corner-radius clips the image.
        let _: () = unsafe { msg_send![&this, setWantsLayer: true] };
        let layer: Retained<NSObject> = unsafe { msg_send_id![&this, layer] };
        let _: () = unsafe { msg_send![&layer, setMasksToBounds: true] };
        // Default fit is Contain (aspect-fit) — matches the framework-wide
        // `ObjectFit::Contain` default. `apply_style` overrides it when the
        // node's style sets `object_fit`.
        set_gravity(&layer, ObjectFit::Contain);
        this
    }
}

/// `true` when `view` is one of our layer-backed image views — lets the
/// generic `apply_style` path apply `object_fit` only to images. Identified
/// by responding to our private `setImageFromNSImage:` selector, which no
/// other view implements (avoids a `class()` lookup; `respondsToSelector:`
/// is safe on any `NSObject`).
pub(crate) fn is_image_view(view: &NSView) -> bool {
    unsafe { msg_send![view, respondsToSelector: objc2::sel!(setImageFromNSImage:)] }
}

/// Apply an [`ObjectFit`] to an image view's layer (its `contentsGravity`).
/// Called from `apply_style`; no-op if `view` isn't an image view (the
/// caller guards, but double-checking keeps this safe to call broadly).
pub(crate) fn apply_object_fit(view: &NSView, fit: ObjectFit) {
    if !is_image_view(view) {
        return;
    }
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    set_gravity(&layer, fit);
}

fn set_gravity(layer: &NSObject, fit: ObjectFit) {
    let gravity = NSString::from_str(gravity_for(fit));
    let _: () = unsafe { msg_send![layer, setContentsGravity: &*gravity] };
}

/// Decode `source`'s bytes into an `NSImage` and stash by id.
/// Bundled / Remote sources are no-ops here — a future bundle-
/// resource lookup or `NSURLSession` fetch can populate them.
pub(crate) fn register_asset(
    cache: &mut ImageCache,
    id: AssetId,
    kind: AssetTag,
    source: &AssetSource,
) {
    if kind != AssetTag::Image {
        return;
    }
    if cache.contains_key(&id) {
        return;
    }
    let bytes: &[u8] = match source {
        AssetSource::Embedded { bytes, .. } | AssetSource::BundledEmbedded { bytes, .. } => bytes,
        AssetSource::Bundled { .. } | AssetSource::Remote { .. } => return,
    };
    let Some(image) = decode_image_from_bytes(bytes) else {
        return;
    };
    cache.insert(id, image);
}

/// `+[NSImage alloc] -[initWithData:]` against an NSData built from
/// the slice. Same `dataWithBytes:length:` shape as the iOS path,
/// the class copies — the slice can outlive the call.
fn decode_image_from_bytes(bytes: &[u8]) -> Option<Retained<NSObject>> {
    let data: Retained<NSObject> = unsafe {
        msg_send_id![
            objc2::class!(NSData),
            dataWithBytes: bytes.as_ptr() as *const std::ffi::c_void,
            length: bytes.len()
        ]
    };
    let allocated: *mut AnyObject =
        unsafe { msg_send![objc2::class!(NSImage), alloc] };
    if allocated.is_null() {
        return None;
    }
    let inited: *mut AnyObject = unsafe { msg_send![allocated, initWithData: &*data] };
    if inited.is_null() {
        return None;
    }
    unsafe { Retained::from_raw(inited.cast::<NSObject>()) }
}

/// Assign an `NSImage`'s `CGImage` to `view`'s backing layer and record
/// its natural size for measurement. The layer's `contentsGravity`
/// (set at create / by `apply_style`) decides the fit; here we only
/// swap the bitmap.
fn set_layer_image(view: &NSView, image: &NSObject) {
    // `-[NSImage CGImageForProposedRect:context:hints:]` with a NULL rect
    // returns the bitmap at its natural size (+0 autoreleased); the layer
    // retains it on assignment.
    let cg: *mut AnyObject = unsafe {
        msg_send![
            image,
            CGImageForProposedRect: std::ptr::null_mut::<CGRect>(),
            context: std::ptr::null_mut::<AnyObject>(),
            hints: std::ptr::null_mut::<AnyObject>()
        ]
    };
    if cg.is_null() {
        return;
    }
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    let _: () = unsafe { msg_send![&layer, setContents: cg] };

    // Record the natural (point) size so `intrinsicContentSize` reports it.
    let size: CGSize = unsafe { msg_send![image, size] };
    if is_image_view(view) {
        let iv: &ImageView = unsafe { &*(view as *const NSView as *const ImageView) };
        iv.ivars().natural_size.set(size);
    }
    // A new natural size can change layout — invalidate so the next pass
    // re-reads `intrinsicContentSize`.
    let _: () = unsafe { msg_send![view, invalidateIntrinsicContentSize] };
}

/// Create an image view. If `src` resolves to a cached `NSImage`, its
/// bitmap is assigned to the layer; otherwise the view starts empty and
/// (for a remote URL) an async fetch populates it.
pub(crate) fn create_image(
    mtm: MainThreadMarker,
    cache: &ImageCache,
    src: &str,
    _alt: Option<&str>,
) -> MacosNode {
    let view = ImageView::new(mtm);
    let ns_view: Retained<NSView> = Retained::into_super(view);
    if let Some(image) = resolve_nsimage(cache, src) {
        set_layer_image(&ns_view, &image);
    } else if is_remote_url(src) {
        // No embedded asset — fetch the remote URL (web's `<img src>` analog).
        load_url_image_async(&ns_view, src);
    }
    MacosNode::View(ns_view)
}

/// Update an image view's bitmap when its `src` changes reactively.
/// Mirrors the same asset-cache lookup as `create_image`; the fit
/// (`contentsGravity`) is untouched — a `src` swap keeps the fit.
pub(crate) fn update_image_src(node: &MacosNode, cache: &ImageCache, src: &str) {
    let MacosNode::View(view) = node else {
        return;
    };
    if let Some(image) = resolve_nsimage(cache, src) {
        set_layer_image(view, &image);
    } else if is_remote_url(src) {
        load_url_image_async(view, src);
    }
}

/// Look up an `asset://{id}` URL in the cache. Returns `None` for
/// non-sentinel URLs and for ids that haven't been registered.
fn resolve_nsimage(cache: &ImageCache, src: &str) -> Option<Retained<NSObject>> {
    let rest = src.strip_prefix(ASSET_URL_PREFIX)?;
    let id_value: u64 = rest.parse().ok()?;
    cache.get(&AssetId(id_value)).cloned()
}

/// `true` for `http(s)://` sources the URL loader fetches.
fn is_remote_url(src: &str) -> bool {
    src.starts_with("http://") || src.starts_with("https://")
}

/// `+[NSImage alloc] -[initWithData:]` from a raw `NSData` pointer (the
/// `NSURLSession` completion hands one back). Same init shape as
/// `decode_image_from_bytes` without re-wrapping the bytes.
fn nsimage_from_data_ptr(data: *mut AnyObject) -> Option<Retained<NSObject>> {
    let allocated: *mut AnyObject = unsafe { msg_send![objc2::class!(NSImage), alloc] };
    if allocated.is_null() {
        return None;
    }
    let inited: *mut AnyObject = unsafe { msg_send![allocated, initWithData: data] };
    if inited.is_null() {
        return None;
    }
    unsafe { Retained::from_raw(inited.cast::<NSObject>()) }
}

/// Fetch a remote image URL asynchronously and assign it to `view`'s layer
/// once it arrives. macOS has no built-in URL→view image loading (unlike
/// web's `<img src>`), so we drive `NSURLSession.sharedSession`'s data task:
/// its completion handler runs on a background queue, decodes the bytes into
/// an `NSImage`, then hops the layer assignment to the main thread (CALayer /
/// NSView are main-thread-only). The data task retains the completion block —
/// which retains `view` — until the fetch finishes, so the view outlives an
/// in-flight load even if briefly detached.
fn load_url_image_async(view: &Retained<NSView>, url_str: &str) {
    let ns_url_str = NSString::from_str(url_str);
    let url: *mut AnyObject =
        unsafe { msg_send![objc2::class!(NSURL), URLWithString: &*ns_url_str] };
    if url.is_null() {
        return;
    }
    let session: *mut AnyObject =
        unsafe { msg_send![objc2::class!(NSURLSession), sharedSession] };
    if session.is_null() {
        return;
    }
    let view = view.clone();
    let completion = RcBlock::new(
        move |data: *mut AnyObject, _response: *mut AnyObject, _error: *mut AnyObject| {
            // Runs on a background queue. Crash-loud on panic — the block drains
            // through libdispatch's `extern "C"` boundary, so an unwind past it
            // would abort with no message (project policy: log + abort).
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                if data.is_null() {
                    return;
                }
                let Some(image) = nsimage_from_data_ptr(data) else {
                    return;
                };
                // CALayer/NSView are main-thread-only — hop the assignment to
                // main via the `setImageFromNSImage:` selector (which calls
                // `set_layer_image`). `performSelectorOnMainThread:` retains
                // `image` until it runs.
                let _: () = unsafe {
                    msg_send![
                        &view,
                        performSelectorOnMainThread: objc2::sel!(setImageFromNSImage:),
                        withObject: &*image,
                        waitUntilDone: false
                    ]
                };
            }));
            if result.is_err() {
                eprintln!("[backend-macos] image URL completion handler panicked");
                std::process::abort();
            }
        },
    );
    let task: *mut AnyObject = unsafe {
        msg_send![session, dataTaskWithURL: url, completionHandler: &*completion]
    };
    if task.is_null() {
        return;
    }
    let _: () = unsafe { msg_send![task, resume] };
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2::rc::Retained;
    use objc2::msg_send_id;
    use objc2_app_kit::NSView;
    use objc2_foundation::{CGRect, CGPoint, CGSize};

    // Reads a view's backing-layer `contentsGravity` back as a Rust String.
    fn layer_gravity(view: &NSView) -> String {
        let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
        let g: Retained<NSString> = unsafe { msg_send_id![&layer, contentsGravity] };
        g.to_string()
    }

    // `apply_object_fit` maps each `ObjectFit` to the correct CALayer
    // `contentsGravity` on a real image view — the objc-runtime path that
    // unit-level style tests can't reach. Cover → `resizeAspectFill` is the
    // whole point (NSImageView can't express it); this proves the layer path
    // does. Runs only on the main thread (AppKit views require it).
    #[test]
    fn apply_object_fit_sets_layer_contents_gravity() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        if unsafe { pthread_main_np() } == 0 {
            eprintln!("skipping apply_object_fit_sets_layer_contents_gravity: not on main thread");
            return;
        }
        let mtm = unsafe { MainThreadMarker::new_unchecked() };

        let view: Retained<NSView> = Retained::into_super(ImageView::new(mtm));
        // Our image view is identified by responding to the private selector.
        assert!(is_image_view(&view), "ImageView must be recognized as an image view");
        // Default fit set at create time is Contain (aspect-fit).
        assert_eq!(layer_gravity(&view), "resizeAspect");

        apply_object_fit(&view, ObjectFit::Cover);
        assert_eq!(layer_gravity(&view), "resizeAspectFill", "Cover → aspect-fill");

        apply_object_fit(&view, ObjectFit::Fill);
        assert_eq!(layer_gravity(&view), "resize", "Fill → stretch");

        apply_object_fit(&view, ObjectFit::Contain);
        assert_eq!(layer_gravity(&view), "resizeAspect", "Contain → aspect-fit");
    }

    // A plain NSView is NOT an image view, so `apply_object_fit` leaves it
    // untouched — the generic `apply_style` path must not clobber non-image
    // views' layers.
    #[test]
    fn plain_view_is_not_an_image_view() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        if unsafe { pthread_main_np() } == 0 {
            eprintln!("skipping plain_view_is_not_an_image_view: not on main thread");
            return;
        }
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let frame = CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(10.0, 10.0));
        let view: Retained<NSView> =
            unsafe { msg_send_id![mtm.alloc::<NSView>(), initWithFrame: frame] };
        assert!(!is_image_view(&view), "a plain NSView is not one of our image views");
    }
}
