//! `Primitive::Image` — `UIImageView` backed by `UIImage(data:)`.
//!
//! Two paths land here:
//!
//! - **URL source** (`image("https://...")`): the framework's
//!   `create_image` gets the URL string; on iOS we don't fetch — the
//!   image is mounted empty and the caller can use a future `image`
//!   primitive op (TBD) to set the bitmap. URL-driven images on iOS
//!   are out of scope for v1.
//! - **Asset source** (`image_asset(LOGO)`): the walker first calls
//!   `register_asset(id, AssetTag::Image, source)` — we decode the
//!   bytes into a `UIImage` and stash it keyed by id. Then
//!   `create_image` runs with `src = "asset://{id}"`; we look up the
//!   `UIImage` and assign it to a fresh `UIImageView`.
//!
//! `UIImage(data:)` natively decodes PNG, JPG, HEIC, GIF, TIFF, BMP,
//! WebP (on iOS 14+), and ICO. SVG is **not** supported by
//! `UIImage(data:)` — for SVG assets, raster (e.g. PNG) before
//! `embed_asset!` or implement an SVG renderer in a follow-up.

use std::collections::HashMap;

use framework_core::{AssetId, AssetSource, AssetTag};
use objc2::msg_send;
use objc2::msg_send_id;
use objc2::rc::Retained;
use objc2_foundation::{CGPoint, CGRect, CGSize, MainThreadMarker, NSObject};
use objc2_ui_kit::UIView;

use super::IosNode;

/// `UIImage` cache keyed by [`AssetId`]. Filled by
/// `register_asset` (Embedded → `UIImage(data:)`); queried by
/// `create_image` when the `src` is an `asset://{id}` sentinel.
///
/// Holds an `NSObject` rather than a typed `UIImage` because objc2's
/// `UIImage` binding isn't currently re-exported from the local
/// frameworks; the wrapper goes through `msg_send_id!` directly.
pub(crate) type ImageCache = HashMap<AssetId, Retained<NSObject>>;

const ASSET_URL_PREFIX: &str = "asset://";

/// Decode `source`'s bytes into a `UIImage` and stash by id. Bundled
/// / Remote sources are recorded with `None`; future work can add
/// bundle-resource lookup and `NSURLSession` fetches.
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
        AssetSource::Embedded { bytes, .. } => bytes,
        AssetSource::Bundled { .. } | AssetSource::Remote { .. } => {
            // Bundled/Remote on iOS need a bundle-resource lookup or
            // an async fetch. Skip for now so `create_image` falls
            // through to the empty-image branch; the framework's
            // image primitive is still well-formed.
            return;
        }
    };
    let Some(image) = decode_image_from_bytes(bytes) else {
        return;
    };
    cache.insert(id, image);
}

/// `[UIImage imageWithData:nsdata]`. Returns `None` if the bytes
/// can't be decoded as any format UIImage natively supports.
///
/// The decoded image is forced to `UIImageRenderingModeAlwaysOriginal`
/// (= 1) so it renders its actual pixels — without this, an image
/// view nested under a control that uses `tintColor` (UIButton,
/// nav-bar items, etc.) can paint the image as a flat-tinted
/// silhouette.
fn decode_image_from_bytes(bytes: &[u8]) -> Option<Retained<NSObject>> {
    // Build the NSData via `+[NSData dataWithBytes:length:]` so we
    // don't depend on objc2-foundation's constructor surface (which
    // varies by version). The class copies the bytes — the slice can
    // outlive the call without leaks or dangling.
    let data: Retained<NSObject> = unsafe {
        msg_send_id![
            objc2::class!(NSData),
            dataWithBytes: bytes.as_ptr() as *const std::ffi::c_void,
            length: bytes.len()
        ]
    };
    let image: Option<Retained<NSObject>> = unsafe {
        msg_send_id![objc2::class!(UIImage), imageWithData: &*data]
    };
    image.map(|img| {
        // 1 = UIImageRenderingModeAlwaysOriginal
        let original: Retained<NSObject> =
            unsafe { msg_send_id![&img, imageWithRenderingMode: 1i64] };
        original
    })
}

/// Create a `UIImageView`. If `src` is `asset://{id}` and the id is
/// in `cache`, the view's `image` is set to the decoded `UIImage`;
/// otherwise the view starts empty and the caller can update later
/// via `update_image_src`.
pub(crate) fn create_image(
    mtm: MainThreadMarker,
    cache: &ImageCache,
    src: &str,
    _alt: Option<&str>,
) -> IosNode {
    let _ = mtm;
    let cls = objc2::class!(UIImageView);
    let view: Retained<UIView> = unsafe {
        // `initWithFrame:CGRectZero` — Taffy assigns the real frame
        // in the layout pass. Inline alloc+init matches the icon
        // module's pattern (objc2's msg_send_id wants alloc piped
        // directly into init, not via a bound variable).
        msg_send_id![
            msg_send_id![cls, alloc],
            initWithFrame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0))
        ]
    };
    if let Some(image) = resolve_uiimage(cache, src) {
        let _: () = unsafe { msg_send![&view, setImage: &*image] };
    }
    // `contentMode = UIViewContentModeScaleAspectFit` (= 1) so the
    // bitmap scales to fit the layout frame without distortion. The
    // default `scaleToFill` (= 0) stretches arbitrarily and looks
    // wrong as soon as Taffy gives the view a non-square frame.
    let _: () = unsafe { msg_send![&view, setContentMode: 1i64] };
    // Pin tintAdjustmentMode to Normal so a dimmed-tint ancestor
    // (e.g. a modal presentation context) can't repaint the image as
    // a flat silhouette. 1 = UIViewTintAdjustmentModeNormal.
    let _: () = unsafe { msg_send![&view, setTintAdjustmentMode: 1i64] };
    IosNode::View(view)
}

/// Update a `UIImageView`'s image when its `src` changes reactively.
/// Mirrors the same `asset://{id}` decoding as `create_image`.
pub(crate) fn update_image_src(node: &IosNode, cache: &ImageCache, src: &str) {
    let IosNode::View(view) = node else {
        return;
    };
    if let Some(image) = resolve_uiimage(cache, src) {
        let _: () = unsafe { msg_send![view, setImage: &*image] };
    }
    // No image found — leave the view as it was. A future
    // ImageOps::reset() could explicitly clear via
    // `setImage:nil`; today the URL/asset path is fire-and-forget.
}

/// Look up an `asset://{id}` URL in the cache. Returns `None` for
/// non-sentinel URLs and for ids that haven't been registered.
fn resolve_uiimage(cache: &ImageCache, src: &str) -> Option<Retained<NSObject>> {
    let rest = src.strip_prefix(ASSET_URL_PREFIX)?;
    let id_value: u64 = rest.parse().ok()?;
    cache.get(&AssetId(id_value)).cloned()
}

