//! `Element::Image` — `UIImageView` backed by `UIImage(data:)`.
//!
//! The `src` contract is `docs/image-src.md`; the decisions (routing,
//! bundled-asset paths, what a fetch result means, dropping superseded
//! fetches) are `backend_apple_core::image_load_policy`, shared with macOS.
//! This file performs them with UIKit:
//!
//! - **URL source** (`image("https://...")`): fetched with `NSURLSession`
//!   (`apple_image::fetch_image`) and assigned on the main thread when it
//!   arrives. A network error, a non-2xx status, an empty body or an
//!   unparseable URL fires `on_error`. `http` needs an ATS exception.
//! - **Asset source** (`image_asset(LOGO)`): the walker first calls
//!   `register_asset(id, AssetTag::Image, source)`, which decodes
//!   `embed_asset!` bytes, reads an `asset!` file from
//!   `<App>.app/assets/<path>`, or records an `AssetSource::Remote` URL.
//!   Then `create_image` runs with `src = "asset://{id}"` and shows it.
//! - **`data:` URI** (`image("data:image/svg+xml,…")`): decoded
//!   synchronously, base64 or percent-encoded.
//! - Anything else is rejected: `on_error` fires and debug builds log why.
//!
//! Every `src` applied to a view starts a new load generation; a fetch that
//! completes for an older `src` is dropped, so a slow response never
//! replaces a newer image.
//!
//! `UIImage(data:)` natively decodes PNG, JPEG, HEIC, GIF (first frame),
//! TIFF, BMP, WebP (iOS 14+; the framework's floor is iOS 16) and ICO, but
//! not SVG. Every path (asset bytes,
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

use backend_apple_core::image_load_policy::{
    self as policy, AssetPlan, ImageLoadState, LoadOutcome, LoadTicket, SrcFailure, SrcPlan,
};
use backend_apple_core::image_source::{self as apple_image, Fetched};
use backend_image_source::{self as image_source, DecodedSource, SvgImage};

use runtime_shared::{
    AssetId, AssetSource, AssetTag, ImageErrorHandler, ImageLoadEvent, ImageLoadHandler,
};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{declare_class, msg_send, msg_send_id, mutability, ClassType, DeclaredClass};
use objc2_foundation::{CGFloat, CGPoint, CGRect, CGSize, MainThreadMarker, NSObject};
use objc2_ui_kit::{UIImageView, UIView};

use super::IosNode;

/// Decoded-asset cache keyed by [`AssetId`]. Filled by
/// `register_asset`; queried by `create_image` when the `src` is an
/// `asset://{id}` sentinel.
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
    /// `AssetSource::Remote`: fetched like an `https://` `src`, per view.
    Remote(&'static str),
    /// Registered, but its bytes are missing (an `asset!` file the build
    /// didn't ship) or don't decode. Showing it fires `on_error`.
    Unavailable,
}


/// `on_load` / `on_error` observer state for an image view. Held in the
/// [`ImageView`] subclass's ivars so the async URL loader — which retains
/// the view — can fire the framework handlers when the bitmap arrives (or
/// fails), and so an installed handler can fire immediately if the bitmap
/// already decoded. Mirrors the macOS `ImageViewIvars`.
pub struct ImageViewIvars {
    on_load: RefCell<Option<ImageLoadHandler>>,
    on_error: RefCell<Option<ImageErrorHandler>>,
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
    /// Load generation + the current load's outcome. A fetch completion
    /// whose ticket is no longer current is dropped; a late-installed
    /// handler is replayed from the outcome only (see
    /// `image_load_policy::ImageLoadState`).
    load: ImageLoadState,
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
        // Identity marker: `is_our_image_view` checks for this selector
        // (no stock `UIImageView` responds to it). Calling it reports a
        // load failure, as its name says.
        #[method(imageLoadFailed)]
        fn image_load_failed(&self) {
            report_load_failed(self);
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
            current_src: RefCell::new(String::new()),
            svg: RefCell::new(None),
            svg_raster_scale: Cell::new(0.0),
            load: ImageLoadState::new(),
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
/// the bitmap's natural size. Used by the synchronous paths and the async
/// URL loader's main-thread completion.
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
        let (width, height) = (size.width as f32, size.height as f32);
        iv.ivars().load.loaded(width, height);
        // Clone out before calling: the handler may re-enter this view.
        let handler = iv.ivars().on_load.borrow().clone();
        if let Some(h) = handler {
            h(&ImageLoadEvent { width, height });
        }
    }
}

/// Install the framework `on_load` observer, replaying once if the
/// CURRENT load already succeeded (the embedded-asset order, where
/// `create_image` assigns before the walker installs the handler). Replay
/// reads the load outcome, not "the view holds an image": while a new `src`
/// loads the previous bitmap is still up, and replaying it would deliver a
/// second `on_load` when the new load lands.
pub(crate) fn install_load_handler(node: &IosNode, handler: ImageLoadHandler) {
    let IosNode::View(view) = node else { return };
    if !is_our_image_view(view) {
        return;
    }
    let iv: &ImageView = unsafe { &*(Retained::as_ptr(view) as *const ImageView) };
    *iv.ivars().on_load.borrow_mut() = Some(handler.clone());
    if let LoadOutcome::Loaded { width, height } = iv.ivars().load.outcome() {
        handler(&ImageLoadEvent { width, height });
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
    let already = iv.ivars().load.outcome() == LoadOutcome::Failed;
    *iv.ivars().on_error.borrow_mut() = Some(handler.clone());
    if already {
        handler();
    }
}

/// Resolve an image asset and stash it by id: `embed_asset!` bytes are
/// decoded now, an `asset!` file is read from `<App>.app/assets/<path>`
/// and decoded now (once per asset, not per view), and an
/// `AssetSource::Remote` URL is recorded for each view to fetch. A file the
/// build didn't ship, or bytes that don't decode, is cached as
/// `Unavailable` so the view fires `on_error` with that reason.
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
    let cached = match policy::asset_plan(source) {
        AssetPlan::Bytes(bytes) => decode_asset_bytes(bytes),
        AssetPlan::BundleFile(path) => match apple_image::read_bundled_asset(path) {
            Some(bytes) => decode_asset_bytes(&bytes),
            None => {
                backend_apple_core::apple_log(&format!(
                    "[backend-ios] image: asset {path:?} is not in the app bundle (expected <App>.app/assets/{path})"
                ));
                CachedImage::Unavailable
            }
        },
        AssetPlan::Remote(url) => CachedImage::Remote(url),
    };
    cache.insert(id, cached);
}

fn decode_asset_bytes(bytes: &[u8]) -> CachedImage {
    match image_source::decode_bytes(bytes) {
        Some(DecodedSource::Svg(svg)) => CachedImage::Svg(Rc::new(svg)),
        Some(DecodedSource::Bitmap(bytes)) => match decode_image_from_bytes(&bytes) {
            Some(image) => CachedImage::Bitmap(image),
            None => CachedImage::Unavailable,
        },
        None => CachedImage::Unavailable,
    }
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

/// Resolve `src` and put it on `view`, following `docs/image-src.md`
/// through `image_load_policy::plan_src`: a cached asset or `data:` URI
/// synchronously, a remote URL asynchronously, anything else `on_error`.
/// Every call starts a new load generation, which supersedes any fetch
/// still in flight for this view.
fn apply_src(view: &Retained<UIView>, cache: &ImageCache, src: &str) {
    let ticket = begin_load(view);
    match policy::plan_src(src) {
        SrcPlan::Asset(id) => match cache.get(&AssetId(id)).cloned() {
            Some(CachedImage::Bitmap(image)) => set_image_and_notify(view, &image),
            Some(CachedImage::Svg(svg)) => show_svg_on(view, svg),
            Some(CachedImage::Remote(url)) => load_url_image_async(view, url, ticket),
            Some(CachedImage::Unavailable) => fail_load(view, src, SrcFailure::AssetUnavailable),
            None => fail_load(view, src, SrcFailure::UnknownAsset),
        },
        SrcPlan::Decoded(DecodedSource::Svg(svg)) => show_svg_on(view, Rc::new(svg)),
        SrcPlan::Decoded(DecodedSource::Bitmap(bytes)) => match decode_image_from_bytes(&bytes) {
            Some(image) => set_image_and_notify(view, &image),
            None => fail_load(view, src, SrcFailure::UndecodableBitmap),
        },
        SrcPlan::Remote(url) => load_url_image_async(view, url, ticket),
        SrcPlan::Fail(failure) => fail_load(view, src, failure),
    }
}

/// Start a new load on `view`: bump its generation and forget the previous
/// outcome (a late handler must be replayed THIS `src`'s outcome).
fn begin_load(view: &UIView) -> Option<LoadTicket> {
    if !is_our_image_view(view) {
        return None;
    }
    let iv: &ImageView = unsafe { &*(view as *const UIView as *const ImageView) };
    Some(iv.ivars().load.begin())
}

/// Log why `src` produced no image, clear the view, and fire `on_error`.
/// A rejected `src` logs through `image_source::warn_rejected` (debug
/// builds), the same line every backend prints.
fn fail_load(view: &UIView, src: &str, failure: SrcFailure) {
    match failure {
        SrcFailure::Rejected(reason) => image_source::warn_rejected(src, reason),
        other => backend_apple_core::apple_log(&format!(
            "[backend-ios] image: {} ({}) — firing on_error",
            other.describe(),
            &src[..src.floor_char_boundary(48)]
        )),
    }
    if is_our_image_view(view) {
        let iv: &ImageView = unsafe { &*(view as *const UIView as *const ImageView) };
        // A failed `src` shows nothing, like a broken `<img>`; leaving the
        // previous image up would also let a late `on_load` install fire.
        iv.ivars().svg.borrow_mut().take();
        iv.ivars().svg_raster_scale.set(0.0);
        let _: () = unsafe { msg_send![view, setImage: std::ptr::null_mut::<AnyObject>()] };
        report_load_failed(iv);
    }
}

/// Record a failed load (so a late `on_error` is replayed it) and fire the
/// installed handler.
fn report_load_failed(iv: &ImageView) {
    iv.ivars().load.failed();
    // Clone out before calling: the handler may re-enter this view.
    let handler = iv.ivars().on_error.borrow().clone();
    if let Some(h) = handler {
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

/// `[UIImage imageWithData:]` from a fetched `NSData`, forced to
/// `AlwaysOriginal` rendering like `decode_image_from_bytes` so a tinting
/// ancestor can't flatten it. Runs on the session's background queue
/// (`UIImage` creation is thread-safe; assigning it to a view is not).
fn uiimage_from_nsdata(data: &AnyObject) -> Option<Retained<NSObject>> {
    let image: Option<Retained<NSObject>> =
        unsafe { msg_send_id![objc2::class!(UIImage), imageWithData: data] };
    // 1 = UIImageRenderingModeAlwaysOriginal
    image.map(|img| unsafe { msg_send_id![&img, imageWithRenderingMode: 1i64] })
}

/// Fetch a remote image and show it on `view` when it arrives (web's
/// `<img src>` analog; iOS has no URL-backed image view). The fetch and the
/// main-thread hop are `apple_image::fetch_image`, shared with macOS; this
/// supplies the `UIImage` decoder and applies the result, unless `ticket`
/// has been superseded by a newer `src` in the meantime, in which case the
/// result is dropped (no image, no `on_load` / `on_error`).
fn load_url_image_async(view: &Retained<UIView>, url: &str, ticket: Option<LoadTicket>) {
    let view = view.clone();
    let src = url.to_string();
    apple_image::fetch_image(url, uiimage_from_nsdata, move |result| {
        if !is_our_image_view(&view) {
            return;
        }
        let iv: &ImageView = unsafe { &*(Retained::as_ptr(&view) as *const ImageView) };
        if !ticket.is_some_and(|t| iv.ivars().load.is_current(t)) {
            return;
        }
        match result {
            Ok(Fetched::Bitmap(image)) => set_image_and_notify(&view, &image),
            Ok(Fetched::Svg(data)) => {
                match SvgImage::parse(unsafe { apple_image::nsdata_bytes(&data) }) {
                    Some(svg) => show_svg(iv, Rc::new(svg)),
                    None => fail_load(&view, &src, SrcFailure::Fetch(policy::FetchError::UndecodableSvg)),
                }
            }
            Err(failure) => fail_load(&view, &src, failure),
        }
    });
}
