//! `Element::Image` — `NSImageView` backed by `NSImage(data:)`.
//!
//! Mirrors the iOS image module shape. Two paths:
//!
//! - **URL source** (`image("https://...")`): mounted empty; future
//!   `NSURLSession` fetch can populate the image, same as iOS's
//!   URL-source TODO.
//! - **Asset source** (`image_asset(LOGO)`): the walker calls
//!   `register_asset(id, AssetTag::Image, source)` first; we decode
//!   the bytes into an `NSImage` and cache by id. `create_image`
//!   then receives `src = "asset://{id}"`, looks up the cached
//!   image, and assigns it to a fresh `NSImageView`.
//!
//! `NSImage(data:)` natively decodes PNG, JPG, TIFF, BMP, GIF, ICO,
//! and (on macOS 12+) HEIF and WebP. SVG is not supported; rasterize
//! before embedding.

use std::collections::HashMap;

use runtime_core::{AssetId, AssetSource, AssetTag};
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSView;
use objc2_foundation::NSObject;

use super::MacosNode;

/// `NSImage` cache keyed by [`AssetId`]. Filled by `register_asset`
/// (Embedded → `NSImage(data:)`); queried by `create_image` when
/// `src` is an `asset://{id}` sentinel. Held as `NSObject` because
/// `objc2-app-kit`'s `NSImage` re-export comes through the runtime
/// dispatch, not as a typed pointer we own.
pub(crate) type ImageCache = HashMap<AssetId, Retained<NSObject>>;

const ASSET_URL_PREFIX: &str = "asset://";

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
///
/// objc2's `msg_send_id!` macro path expects `Allocated<T>` for
/// alloc/init; NSImage isn't a typed re-export we own, so we use
/// raw `msg_send` plus `Retained::from_raw` (mirrors the same
/// pattern used by maps-ios for MKMapView and webview-ios for
/// WKWebView).
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

/// Create an `NSImageView`. If `src` resolves to a cached `NSImage`,
/// the view's `image` is set; otherwise the view starts empty.
pub(crate) fn create_image(
    cache: &ImageCache,
    src: &str,
    _alt: Option<&str>,
) -> MacosNode {
    // Raw alloc + init — same rationale as `decode_image_from_bytes`
    // above: NSImageView isn't a typed re-export here, so we step
    // through raw pointers and wrap once at the end.
    let allocated: *mut AnyObject =
        unsafe { msg_send![objc2::class!(NSImageView), alloc] };
    let inited: *mut AnyObject = unsafe { msg_send![allocated, init] };
    let view: Retained<NSView> = unsafe {
        Retained::from_raw(inited.cast::<NSView>())
            .expect("NSImageView init returned nil")
    };
    if let Some(image) = resolve_nsimage(cache, src) {
        let _: () = unsafe { msg_send![&view, setImage: &*image] };
    }
    // NSImageScaleProportionallyDown (= 2): scale down to fit, never
    // up. The iOS `UIViewContentModeScaleAspectFit` (1) is the
    // analog — both scale-to-fit-bounds without distortion. Down-
    // scaling matches macOS app convention: assets rendered at @2x
    // shouldn't blur when shown at @1x sizes, but a tiny asset
    // shouldn't be blown up into a fuzzy hero either.
    let _: () = unsafe { msg_send![&view, setImageScaling: 2i64] };
    MacosNode::View(view)
}

/// Update an `NSImageView`'s image when its `src` changes
/// reactively. Mirrors the same asset-cache lookup as `create_image`.
pub(crate) fn update_image_src(node: &MacosNode, cache: &ImageCache, src: &str) {
    let MacosNode::View(view) = node else {
        return;
    };
    if let Some(image) = resolve_nsimage(cache, src) {
        let _: () = unsafe { msg_send![view, setImage: &*image] };
    }
}

/// Look up an `asset://{id}` URL in the cache. Returns `None` for
/// non-sentinel URLs and for ids that haven't been registered.
fn resolve_nsimage(cache: &ImageCache, src: &str) -> Option<Retained<NSObject>> {
    let rest = src.strip_prefix(ASSET_URL_PREFIX)?;
    let id_value: u64 = rest.parse().ok()?;
    cache.get(&AssetId(id_value)).cloned()
}
