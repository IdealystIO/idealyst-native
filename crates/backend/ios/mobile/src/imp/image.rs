//! `Element::Image` — `UIImageView` backed by `UIImage(data:)`.
//!
//! Two paths land here:
//!
//! - **URL source** (`image("https://...")`): the framework's
//!   `create_image` gets the URL string; we fetch it asynchronously via
//!   `NSURLSession` (`load_url_image_async`) and assign the decoded
//!   `UIImage` on the main thread when it arrives — the web `<img src>`
//!   analog. `http(s)://` only; `http` needs an ATS exception.
//! - **Asset source** (`image_asset(LOGO)`): the walker first calls
//!   `register_asset(id, AssetTag::Image, source)` — we decode the
//!   bytes into a `UIImage` and stash it keyed by id. Then
//!   `create_image` runs with `src = "asset://{id}"`; we look up the
//!   `UIImage` and assign it to a fresh `UIImageView`.
//!
//! - **`data:` URI** (`image("data:image/svg+xml,…")`): decoded
//!   synchronously in `create_image` / `update_image_src` via
//!   `backend_image_source::parse_data_uri` (base64 or
//!   percent-encoded).
//!
//! `UIImage(data:)` natively decodes PNG, JPG, HEIC, GIF, TIFF, BMP,
//! WebP (on iOS 14+), and ICO — but not SVG. Every path (asset bytes,
//! `data:` URI, fetched URL) sniffs for SVG first and, if found, keeps
//! the parsed document on the view and rasterizes it with resvg at the
//! view's displayed pixel size; `layoutSubviews` re-rasterizes when the
//! bounds or `contentScaleFactor` change, so a vector logo stays sharp at
//! any size. The `UIImage`'s point size stays the SVG's intrinsic size, so
//! measurement and `on_load` don't move on re-raster. A source that can't
//! be decoded fires `on_error` and logs a warning instead of leaving a
//! silently blank slot. Shared with macOS and Android — see `backend_image_source`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use backend_apple_core::image_source as apple_image;
use backend_image_source::{self as image_source, DecodedSource, SvgImage};

use runtime_shared::{
    AssetId, AssetSource, AssetTag, ImageErrorHandler, ImageLoadEvent, ImageLoadHandler,
};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSObject, NSString};
use objc2_ui_kit::{UIImageView, UIView};

use super::IosNode;

/// Decoded-asset cache keyed by [`AssetId`]. Filled by
/// `register_asset` (Embedded → `UIImage(data:)`, or a parsed SVG);
/// queried by `create_image` when the `src` is an `asset://{id}` sentinel.
pub(crate) type ImageCache = HashMap<AssetId, CachedImage>;

/// One decoded asset. A bitmap is decoded once; an SVG is kept as the
/// parsed document because each view rasterizes it at its own size.
///
/// The bitmap is an `NSObject` rather than a typed `UIImage` because objc2's
/// `UIImage` binding isn't currently re-exported from the local frameworks;
/// the wrapper goes through `msg_send_id!` directly.
#[derive(Clone)]
pub(crate) enum CachedImage {
    Bitmap(Retained<NSObject>),
    Svg(Rc<SvgImage>),
}


/// `on_load` / `on_error` observer state for an image view. Held in the
/// [`ImageView`] subclass's ivars so the async URL loader — which retains
/// the view — can fire the framework handlers when the bitmap arrives (or
/// fails), and so an installed handler can fire immediately if the bitmap
/// already decoded. Mirrors the macOS `ImageViewIvars`.
pub struct ImageViewIvars {
    on_load: RefCell<Option<ImageLoadHandler>>,
    on_error: RefCell<Option<ImageErrorHandler>>,
    /// Set once an async load has failed, so an `on_error` installed after
    /// the failure still fires.
    errored: Cell<bool>,
    /// The `src` currently displayed / in-flight. Guards `update_image_src`
    /// against re-loading an unchanged URL — the walker's reactive-`src`
    /// Effect fires once at mount with the URL `create_image` already
    /// loaded, which would otherwise start a duplicate `NSURLSession` fetch
    /// (and a duplicate `on_load`).
    current_src: RefCell<String>,
    /// The SVG document currently displayed, if the source was SVG. Kept so
    /// `layoutSubviews` can re-rasterize at the new size / scale.
    svg: RefCell<Option<Rc<SvgImage>>>,
    /// Pixels per intrinsic point of the current SVG raster (0 = none yet).
    svg_raster_scale: Cell<f64>,
}

declare_class!(
    /// `UIImageView` subclass carrying `on_load` / `on_error` observers.
    /// Behaves exactly like `UIImageView` (so `apply_object_fit`'s
    /// `isKindOfClass: UIImageView` check still matches); the subclass only
    /// adds the load-notification plumbing.
    pub struct ImageView;

    unsafe impl ClassType for ImageView {
        type Super = UIImageView;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "IdealystImageView";
    }

    impl DeclaredClass for ImageView {
        type Ivars = ImageViewIvars;
    }

    unsafe impl ImageView {
        // Main-thread hop target for the async URL loader's success branch.
        // Sets the image AND fires `on_load` — distinct from a plain
        // `setImage:` so ad-hoc UIKit `setImage:` calls don't spuriously
        // notify. Mirrors macOS `setImageFromNSImage:`.
        #[method(setImageFromUIImage:)]
        fn set_image_from_ui_image(&self, image: &NSObject) {
            let view: &UIView = self;
            set_image_and_notify(view, image);
        }

        // Main-thread hop target for the async loader's failure branch
        // (null data / undecodable bytes). Fires `on_error`; records the
        // failure so a late-installed handler still sees it.
        #[method(imageLoadFailed)]
        fn image_load_failed(&self) {
            report_load_failed(self);
        }

        // Main-thread hop target for the async loader when the fetched bytes
        // are SVG: `UIImage(data:)` can't decode them, and the parsed tree is
        // `!Send`-adjacent per-view state, so the raw `NSData` crosses
        // threads and is parsed + rasterized here.
        #[method(setImageFromSVGData:)]
        fn set_image_from_svg_data(&self, data: &AnyObject) {
            let bytes = unsafe { apple_image::nsdata_bytes(data) };
            match SvgImage::parse(bytes) {
                Some(svg) => show_svg(self, Rc::new(svg)),
                None => report_load_failed(self),
            }
        }

        // Re-rasterize a displayed SVG when the bounds or screen scale
        // changed since the last raster (Taffy assigns the real frame after
        // `create_image`, so the first raster is usually at 0×0 → 1 pt/pt).
        #[method(layoutSubviews)]
        fn layout_subviews(&self) {
            let _: () = unsafe { msg_send![super(self), layoutSubviews] };
            super::ffi_guard::guard_ffi("IdealystImageView layoutSubviews", || {
                let svg = self.ivars().svg.borrow().clone();
                if let Some(svg) = svg {
                    let scale = svg_scale_for(self, &svg);
                    if scale != self.ivars().svg_raster_scale.get() {
                        if let Some(image) = svg_uiimage(&svg, scale) {
                            self.ivars().svg_raster_scale.set(scale);
                            // Plain `setImage:` — the point size is unchanged,
                            // so neither `on_load` nor layout needs to rerun.
                            let _: () = unsafe { msg_send![self, setImage: &*image] };
                        }
                    }
                }
            });
        }
    }
);

impl ImageView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ImageViewIvars {
            on_load: RefCell::new(None),
            on_error: RefCell::new(None),
            errored: Cell::new(false),
            current_src: RefCell::new(String::new()),
            svg: RefCell::new(None),
            svg_raster_scale: Cell::new(0.0),
        });
        // `initWithFrame:CGRectZero` — Taffy assigns the real frame in the
        // layout pass (matches the old raw-`UIImageView` create path).
        unsafe {
            msg_send_id![
                super(this),
                initWithFrame: CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(0.0, 0.0))
            ]
        }
    }
}

/// `true` when `view` is one of *our* image views (responds to the private
/// `imageLoadFailed` selector — no stock `UIImageView` does). Guards the
/// ivar casts. Mirrors macOS's `respondsToSelector:` identity check.
fn is_our_image_view(view: &UIView) -> bool {
    unsafe { msg_send![view, respondsToSelector: objc2::sel!(imageLoadFailed)] }
}

/// Assign `image` to `view` and fire its `on_load` observer (if any) with
/// the bitmap's natural size. Used by both the synchronous asset path and
/// the async URL loader's `setImageFromUIImage:` hop.
fn set_image_and_notify(view: &UIView, image: &NSObject) {
    if is_our_image_view(view) {
        // A bitmap replaces any SVG this view was showing; `show_svg`
        // re-arms these after calling us.
        let iv: &ImageView = unsafe { &*(view as *const UIView as *const ImageView) };
        iv.ivars().svg.borrow_mut().take();
        iv.ivars().svg_raster_scale.set(0.0);
    }
    let _: () = unsafe { msg_send![view, setImage: image] };
    let size: CGSize = unsafe { msg_send![image, size] };
    if is_our_image_view(view) {
        let iv: &ImageView = unsafe { &*(view as *const UIView as *const ImageView) };
        iv.ivars().errored.set(false);
        if let Some(h) = iv.ivars().on_load.borrow().clone() {
            h(&ImageLoadEvent {
                width: size.width as f32,
                height: size.height as f32,
            });
        }
    }
}

/// Install the framework `on_load` observer, firing immediately if the
/// view already holds a decoded bitmap (the embedded-asset order, where
/// `create_image` assigns before the walker installs the handler).
pub(crate) fn install_load_handler(node: &IosNode, handler: ImageLoadHandler) {
    let IosNode::View(view) = node else { return };
    if !is_our_image_view(view) {
        return;
    }
    let iv: &ImageView = unsafe { &*(Retained::as_ptr(view) as *const ImageView) };
    *iv.ivars().on_load.borrow_mut() = Some(handler.clone());
    let image: *mut AnyObject = unsafe { msg_send![view, image] };
    if !image.is_null() {
        let size: CGSize = unsafe { msg_send![image, size] };
        handler(&ImageLoadEvent {
            width: size.width as f32,
            height: size.height as f32,
        });
    }
}

/// Install the framework `on_error` observer, firing immediately if the
/// image already failed to load before the handler was installed.
pub(crate) fn install_error_handler(node: &IosNode, handler: ImageErrorHandler) {
    let IosNode::View(view) = node else { return };
    if !is_our_image_view(view) {
        return;
    }
    let iv: &ImageView = unsafe { &*(Retained::as_ptr(view) as *const ImageView) };
    let already = iv.ivars().errored.get();
    *iv.ivars().on_error.borrow_mut() = Some(handler.clone());
    if already {
        handler();
    }
}

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
        AssetSource::Embedded { bytes, .. } | AssetSource::BundledEmbedded { bytes, .. } => bytes,
        AssetSource::Bundled { .. } | AssetSource::Remote { .. } => {
            // Bundled/Remote on iOS need a bundle-resource lookup or
            // an async fetch. Skip for now so `create_image` falls
            // through to the empty-image branch; the framework's
            // image primitive is still well-formed.
            return;
        }
    };
    let cached = match image_source::decode_bytes(bytes) {
        Some(DecodedSource::Svg(svg)) => CachedImage::Svg(Rc::new(svg)),
        Some(DecodedSource::Bitmap(bytes)) => match decode_image_from_bytes(&bytes) {
            Some(image) => CachedImage::Bitmap(image),
            None => return,
        },
        None => return,
    };
    cache.insert(id, cached);
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
    // Our `UIImageView` subclass carries the `on_load` / `on_error` ivars.
    let view_typed = ImageView::new(mtm);
    // Record the mount src so the walker's reactive-`src` Effect (which
    // fires once with this same URL) is a no-op instead of a duplicate load.
    *view_typed.ivars().current_src.borrow_mut() = src.to_string();
    // `into_super` steps one class up (ImageView → UIImageView → UIView),
    // so apply it twice to land on the `Retained<UIView>` `IosNode` holds.
    let view: Retained<UIView> = Retained::into_super(Retained::into_super(view_typed));
    apply_src(&view, cache, src);
    // Default `contentMode = UIViewContentModeScaleAspectFit` (= 1) — the
    // framework-wide `ObjectFit::Contain` default; `apply_style` overrides
    // it when the node's style sets `object_fit`. The UIView default
    // `scaleToFill` (= 0) stretches arbitrarily and looks wrong as soon as
    // Taffy gives the view a non-square frame.
    let _: () = unsafe { msg_send![&view, setContentMode: 1i64] };
    // Pin tintAdjustmentMode to Normal so a dimmed-tint ancestor
    // (e.g. a modal presentation context) can't repaint the image as
    // a flat silhouette. 1 = UIViewTintAdjustmentModeNormal.
    let _: () = unsafe { msg_send![&view, setTintAdjustmentMode: 1i64] };
    IosNode::View(view)
}

/// `true` when `view` is a `UIImageView` — lets the generic `apply_style`
/// path apply `object_fit` only to images. `isKindOfClass:` is safe on any
/// `UIView`.
pub(crate) fn is_image_view(view: &UIView) -> bool {
    let cls = objc2::class!(UIImageView);
    unsafe { msg_send![view, isKindOfClass: cls] }
}

/// Apply an [`ObjectFit`](runtime_shared::ObjectFit) to an image view via
/// `UIView.contentMode`. No-op on non-image views (the helper guards). For
/// `Cover` (`scaleAspectFill`) the bitmap overflows the frame, so
/// `clipsToBounds` is enabled to crop it — mirroring web `object-fit: cover`
/// + `overflow: hidden`. Called from `apply_style`.
pub(crate) fn apply_object_fit(view: &UIView, fit: runtime_shared::ObjectFit) {
    if !is_image_view(view) {
        return;
    }
    use runtime_shared::ObjectFit;
    // UIViewContentMode: scaleToFill = 0, scaleAspectFit = 1, scaleAspectFill = 2.
    let mode: i64 = match fit {
        ObjectFit::Fill => 0,
        ObjectFit::Contain => 1,
        ObjectFit::Cover => 2,
    };
    let _: () = unsafe { msg_send![view, setContentMode: mode] };
    // Crop the overflow of an aspect-fill (cover) bitmap to the frame.
    if matches!(fit, ObjectFit::Cover) {
        let _: () = unsafe { msg_send![view, setClipsToBounds: true] };
    }
}

/// Update a `UIImageView`'s image when its `src` changes reactively.
/// Mirrors the same `asset://{id}` decoding as `create_image`.
pub(crate) fn update_image_src(node: &IosNode, cache: &ImageCache, src: &str) {
    let IosNode::View(view) = node else {
        return;
    };
    // Skip a redundant re-apply of the URL already displayed / in-flight
    // (see the ivar doc + the mount-time reactive-Effect duplication).
    if is_our_image_view(view) {
        let iv: &ImageView = unsafe { &*(Retained::as_ptr(view) as *const ImageView) };
        if *iv.ivars().current_src.borrow() == src {
            return;
        }
        *iv.ivars().current_src.borrow_mut() = src.to_string();
    }
    apply_src(view, cache, src);
}

/// Resolve `src` and put it on `view`: a cached asset or `data:` URI
/// synchronously, a remote URL asynchronously. An unregistered asset id or
/// an unrecognized scheme leaves the view as it was (a future
/// `ImageOps::reset()` could clear via `setImage:nil`).
fn apply_src(view: &Retained<UIView>, cache: &ImageCache, src: &str) {
    // Same routing as every native backend (`image_source::classify_src`),
    // so one `src` behaves identically everywhere — including the failures:
    // an `asset://` id nothing registered, or a `src` no loader handles,
    // fires `on_error` (and logs) on Android too, instead of a silent blank.
    match image_source::classify_src(src) {
        image_source::SrcKind::Asset(id) => match cache.get(&AssetId(id)).cloned() {
            Some(CachedImage::Bitmap(image)) => set_image_and_notify(view, &image),
            Some(CachedImage::Svg(svg)) => show_svg_on(view, svg),
            None => report_unloadable(view, src, "no registered asset with that id"),
        },
        image_source::SrcKind::DataUri => apply_data_uri(view, src),
        // Fetch the remote URL (web's `<img src>` analog).
        image_source::SrcKind::Remote(url) => load_url_image_async(view, url),
        image_source::SrcKind::Unsupported => {
            report_unloadable(view, src, "unsupported image source")
        }
    }
}

/// Log why `src` can't be shown and fire the view's `on_error`.
fn report_unloadable(view: &UIView, src: &str, why: &str) {
    backend_apple_core::apple_log(&format!(
        "[backend-ios] image: {why} ({}) — firing on_error",
        &src[..src.len().min(48)]
    ));
    if is_our_image_view(view) {
        report_load_failed(unsafe { &*(view as *const UIView as *const ImageView) });
    }
}

/// Decode a `data:` URI and show it, or report the failure.
fn apply_data_uri(view: &UIView, src: &str) {
    let decoded = image_source::parse_data_uri(src).and_then(|uri| {
        Some(match image_source::decode_bytes(&uri.bytes)? {
            DecodedSource::Svg(svg) => CachedImage::Svg(Rc::new(svg)),
            DecodedSource::Bitmap(bytes) => CachedImage::Bitmap(decode_image_from_bytes(&bytes)?),
        })
    });
    match decoded {
        Some(CachedImage::Bitmap(image)) => set_image_and_notify(view, &image),
        Some(CachedImage::Svg(svg)) => show_svg_on(view, svg),
        None => {
            backend_apple_core::apple_log(&format!(
                "[backend-ios] image: could not decode data URI ({}…) — firing on_error",
                &src[..src.len().min(48)]
            ));
            if is_our_image_view(view) {
                report_load_failed(unsafe { &*(view as *const UIView as *const ImageView) });
            }
        }
    }
}

/// Record a failed load: latch `errored` (so a late `on_error` still fires)
/// and fire the installed handler.
fn report_load_failed(iv: &ImageView) {
    iv.ivars().errored.set(true);
    if let Some(h) = iv.ivars().on_error.borrow().clone() {
        h();
    }
}

fn show_svg_on(view: &UIView, svg: Rc<SvgImage>) {
    if is_our_image_view(view) {
        show_svg(unsafe { &*(view as *const UIView as *const ImageView) }, svg);
    }
}

/// Display `svg` on `iv`: rasterize at the current bounds × scale, assign
/// (firing `on_load` with the intrinsic size), and keep the document so
/// `layoutSubviews` can re-rasterize once Taffy sizes the view.
fn show_svg(iv: &ImageView, svg: Rc<SvgImage>) {
    let scale = svg_scale_for(iv, &svg);
    let Some(image) = svg_uiimage(&svg, scale) else {
        report_load_failed(iv);
        return;
    };
    let view: &UIView = iv;
    set_image_and_notify(view, &image);
    *iv.ivars().svg.borrow_mut() = Some(svg);
    iv.ivars().svg_raster_scale.set(scale);
}

/// Raster density for `svg` in `view`'s current bounds and screen scale.
fn svg_scale_for(view: &UIView, svg: &SvgImage) -> f64 {
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    let screen_scale: CGFloat = unsafe { msg_send![view, contentScaleFactor] };
    svg.raster_scale((bounds.size.width as f64, bounds.size.height as f64), screen_scale as f64)
}

/// Rasterize `svg` into a `UIImage` whose point size is the SVG's intrinsic
/// size (`imageWithCGImage:scale:` with the raster's px-per-point scale),
/// forced to `AlwaysOriginal` like the bitmap paths.
fn svg_uiimage(svg: &SvgImage, scale: f64) -> Option<Retained<NSObject>> {
    let raster = svg.rasterize(scale)?;
    let cg = apple_image::cg_image_from_raster(&raster)?;
    let image: Option<Retained<NSObject>> = unsafe {
        msg_send_id![
            objc2::class!(UIImage),
            imageWithCGImage: cg.as_cg_ref(),
            scale: raster.scale as CGFloat,
            orientation: 0i64 // UIImageOrientationUp
        ]
    };
    // 1 = UIImageRenderingModeAlwaysOriginal
    image.map(|img| unsafe { msg_send_id![&img, imageWithRenderingMode: 1i64] })
}

/// SVG sniff on a raw `NSData` pointer (background-thread safe: reads
/// immutable bytes only).
fn nsdata_is_svg(data: *mut AnyObject) -> bool {
    image_source::looks_like_svg(unsafe { apple_image::nsdata_bytes(&*data) })
}

/// `[UIImage imageWithData:]` from a raw `NSData` pointer (the `NSURLSession`
/// completion hands one back), forced to `AlwaysOriginal` rendering like
/// `decode_image_from_bytes` so a tinting ancestor can't flatten it.
fn uiimage_from_data_ptr(data: *mut AnyObject) -> Option<Retained<NSObject>> {
    let image: Option<Retained<NSObject>> =
        unsafe { msg_send_id![objc2::class!(UIImage), imageWithData: data] };
    image.map(|img| {
        // 1 = UIImageRenderingModeAlwaysOriginal
        unsafe { msg_send_id![&img, imageWithRenderingMode: 1i64] }
    })
}

/// Fetch a remote image URL asynchronously and assign it to `view` once it
/// arrives. iOS has no built-in URL→view image loading (unlike web's
/// `<img src>`), so we drive `NSURLSession.sharedSession`'s data task: its
/// completion handler runs on a background queue, decodes the bytes into a
/// `UIImage`, then hops `setImage:` to the main thread via
/// `performSelectorOnMainThread:` (UIView is main-thread-only). The data task
/// retains the completion block — which retains `view` — until the fetch
/// finishes, so the view outlives an in-flight load even if briefly detached.
fn load_url_image_async(view: &Retained<UIView>, url_str: &str) {
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
            backend_apple_core::crash::abort_on_panic("image URL completion handler", || {
                // SVG bytes: `UIImage(data:)` can't decode them — hand the
                // NSData to main, which parses + rasterizes at view size.
                if !data.is_null() && nsdata_is_svg(data) {
                    let _: () = unsafe {
                        msg_send![
                            &view,
                            performSelectorOnMainThread: objc2::sel!(setImageFromSVGData:),
                            withObject: data,
                            waitUntilDone: false
                        ]
                    };
                    return;
                }
                // A network error / 404 yields null data; undecodable bytes
                // yield no `UIImage`. Either way, hop `imageLoadFailed` to
                // main so `on_error` fires on the UI thread.
                let image = if data.is_null() { None } else { uiimage_from_data_ptr(data) };
                let Some(image) = image else {
                    let _: () = unsafe {
                        msg_send![
                            &view,
                            performSelectorOnMainThread: objc2::sel!(imageLoadFailed),
                            withObject: std::ptr::null_mut::<AnyObject>(),
                            waitUntilDone: false
                        ]
                    };
                    return;
                };
                // UIView is main-thread-only — hop the assignment to main via
                // `setImageFromUIImage:` (sets the image AND fires `on_load`).
                // `performSelectorOnMainThread:` retains `image` until it runs.
                let _: () = unsafe {
                    msg_send![
                        &view,
                        performSelectorOnMainThread: objc2::sel!(setImageFromUIImage:),
                        withObject: &*image,
                        waitUntilDone: false
                    ]
                };
            });
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

