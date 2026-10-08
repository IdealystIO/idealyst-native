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
//! - **`data:` URI** (`image("data:image/svg+xml,…")`): decoded
//!   synchronously via `backend_image_source::parse_data_uri`.
//!
//! `NSImage(data:)` natively decodes PNG, JPG, TIFF, BMP, GIF, ICO, and (on
//! macOS 12+) HEIF and WebP. SVG goes through the shared
//! `backend_image_source` path instead (identical to iOS): every
//! source is sniffed, an SVG is parsed once, kept on the view, and
//! rasterized into `layer.contents` at the view's bounds × backing scale —
//! re-rasterized on `setFrameSize:` / a backing-scale change so it stays
//! sharp at any size. `natural_size` stays the SVG's intrinsic size. A source
//! that can't be decoded fires `on_error` and logs a warning.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use backend_apple_core::image_source as apple_image;
use backend_image_source::{self as image_source, DecodedSource, SvgImage};

use runtime_shared::{
    AssetId, AssetSource, AssetTag, ImageErrorHandler, ImageLoadEvent, ImageLoadHandler, ObjectFit,
};
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
pub(crate) type ImageCache = HashMap<AssetId, CachedImage>;

/// One decoded asset. A bitmap is decoded once; an SVG is kept as the
/// parsed document because each view rasterizes it at its own size.
#[derive(Clone)]
pub(crate) enum CachedImage {
    Bitmap(Retained<NSObject>),
    Svg(Rc<SvgImage>),
}


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
    /// Framework `on_load` observer, fired from `set_layer_image` once a
    /// bitmap is assigned (with its natural size). `None` = no observer.
    on_load: RefCell<Option<ImageLoadHandler>>,
    /// Framework `on_error` observer, fired from the async URL loader's
    /// failure branch (via the `imageLoadFailed` main-thread hop).
    on_error: RefCell<Option<ImageErrorHandler>>,
    /// Set once an async load has failed, so an `on_error` handler
    /// installed *after* the failure still fires (mirrors the web
    /// already-errored check).
    errored: Cell<bool>,
    /// The `src` currently displayed (or in-flight). Guards
    /// `update_image_src` against re-applying an unchanged URL — the
    /// walker's reactive-`src` Effect runs once at mount with the same
    /// URL `create_image` already loaded, which would otherwise kick off a
    /// duplicate `NSURLSession` fetch (and a duplicate `on_load`).
    current_src: RefCell<String>,
    /// The SVG document currently displayed, if the source was SVG. Kept so
    /// a resize / backing-scale change can re-rasterize it.
    svg: RefCell<Option<Rc<SvgImage>>>,
    /// Pixels per intrinsic point of the current SVG raster (0 = none yet).
    svg_raster_scale: Cell<f64>,
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

        // Main-thread hop target for the async URL loader's *failure*
        // branch (null data / undecodable bytes). Fires the `on_error`
        // observer on main; records the failure so a late-installed
        // handler still sees it.
        #[method(imageLoadFailed)]
        fn image_load_failed(&self) {
            report_load_failed(self);
        }

        // Main-thread hop target for the async loader when the fetched bytes
        // are SVG (`NSImage(data:)` can't be relied on for them): the raw
        // `NSData` crosses threads and is parsed + rasterized here.
        #[method(setImageFromSVGData:)]
        fn set_image_from_svg_data(&self, data: &AnyObject) {
            match SvgImage::parse(unsafe { apple_image::nsdata_bytes(data) }) {
                Some(svg) => show_svg(self, Rc::new(svg)),
                None => report_load_failed(self),
            }
        }

        // Taffy sets frames with `setFrame:`, which routes through
        // `setFrameSize:` — the resize hook for re-rasterizing an SVG.
        #[method(setFrameSize:)]
        fn set_frame_size(&self, size: CGSize) {
            let _: () = unsafe { msg_send![super(self), setFrameSize: size] };
            rerasterize_svg(self);
        }

        // Moving between a 1× and a 2× display changes the backing scale.
        #[method(viewDidChangeBackingProperties)]
        fn view_did_change_backing_properties(&self) {
            let _: () = unsafe { msg_send![super(self), viewDidChangeBackingProperties] };
            rerasterize_svg(self);
        }
    }
);

impl ImageView {
    pub(crate) fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = mtm.alloc::<Self>();
        let this = this.set_ivars(ImageViewIvars {
            natural_size: Cell::new(CGSize::new(-1.0, -1.0)),
            on_load: RefCell::new(None),
            on_error: RefCell::new(None),
            errored: Cell::new(false),
            current_src: RefCell::new(String::new()),
            svg: RefCell::new(None),
            svg_raster_scale: Cell::new(0.0),
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
    let size: CGSize = unsafe { msg_send![image, size] };
    if is_image_view(view) {
        // A bitmap replaces any SVG this view was showing.
        let iv: &ImageView = unsafe { &*(view as *const NSView as *const ImageView) };
        iv.ivars().svg.borrow_mut().take();
        iv.ivars().svg_raster_scale.set(0.0);
    }
    set_layer_contents(view, cg, size);
}

/// Put `cg` (a `CGImageRef`, passed as the `id` `layer.contents` takes) on
/// `view`'s layer with `size` as the natural point size, and fire `on_load`.
fn set_layer_contents(view: &NSView, cg: *mut AnyObject, size: CGSize) {
    let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
    let _: () = unsafe { msg_send![&layer, setContents: cg] };

    // Record the natural (point) size so `intrinsicContentSize` reports it,
    // and fire the framework `on_load` observer (if any) with it — the
    // bitmap has now decoded and is on screen. Fires per distinct bitmap:
    // `update_image_src`'s src-guard means `set_layer_image` runs once per
    // URL, so `on_load` isn't double-called for the mount's redundant
    // reactive re-apply.
    if is_image_view(view) {
        let iv: &ImageView = unsafe { &*(view as *const NSView as *const ImageView) };
        iv.ivars().natural_size.set(size);
        iv.ivars().errored.set(false);
        if let Some(h) = iv.ivars().on_load.borrow().clone() {
            h(&ImageLoadEvent {
                width: size.width as f32,
                height: size.height as f32,
            });
        }
    }
    // A new natural size can change layout — invalidate so the next pass
    // re-reads `intrinsicContentSize`.
    let _: () = unsafe { msg_send![view, invalidateIntrinsicContentSize] };
}

/// Install the framework `on_load` observer on an image view, firing it
/// immediately if a bitmap has **already** decoded (an embedded asset is
/// assigned synchronously in `create_image`, before the walker installs
/// the handler — this closes that race, mirroring web's `<img>.complete`
/// check). No-op on a non-image view.
pub(crate) fn install_load_handler(node: &MacosNode, handler: ImageLoadHandler) {
    let MacosNode::View(view) = node else { return };
    if !is_image_view(view) {
        return;
    }
    let iv: &ImageView = unsafe { &*(Retained::as_ptr(view) as *const ImageView) };
    let size = iv.ivars().natural_size.get();
    *iv.ivars().on_load.borrow_mut() = Some(handler.clone());
    // Already loaded before install (natural size recorded) → fire now.
    if size.width > 0.0 && size.height > 0.0 {
        handler(&ImageLoadEvent {
            width: size.width as f32,
            height: size.height as f32,
        });
    }
}

/// Install the framework `on_error` observer, firing immediately if the
/// image already failed to load before the handler was installed.
pub(crate) fn install_error_handler(node: &MacosNode, handler: ImageErrorHandler) {
    let MacosNode::View(view) = node else { return };
    if !is_image_view(view) {
        return;
    }
    let iv: &ImageView = unsafe { &*(Retained::as_ptr(view) as *const ImageView) };
    let already = iv.ivars().errored.get();
    *iv.ivars().on_error.borrow_mut() = Some(handler.clone());
    if already {
        handler();
    }
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
    // Record the mount src so the walker's reactive-`src` Effect (which
    // fires once with this same URL) is a no-op instead of a duplicate load.
    *view.ivars().current_src.borrow_mut() = src.to_string();
    let ns_view: Retained<NSView> = Retained::into_super(view);
    apply_src(&ns_view, cache, src);
    MacosNode::View(ns_view)
}

/// Update an image view's bitmap when its `src` changes reactively.
/// Mirrors the same asset-cache lookup as `create_image`; the fit
/// (`contentsGravity`) is untouched — a `src` swap keeps the fit.
pub(crate) fn update_image_src(node: &MacosNode, cache: &ImageCache, src: &str) {
    let MacosNode::View(view) = node else {
        return;
    };
    // Skip a redundant re-apply of the URL already displayed / in-flight.
    // The walker installs a reactive-`src` Effect that runs once at mount
    // with the same URL `create_image` just loaded; without this guard that
    // triggers a second `NSURLSession` fetch and a second `on_load`.
    if is_image_view(view) {
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
/// an unrecognized scheme leaves the view as it was.
fn apply_src(view: &Retained<NSView>, cache: &ImageCache, src: &str) {
    // Same routing as every native backend (`image_source::classify_src`),
    // so one `src` behaves identically everywhere — including the failures:
    // an `asset://` id nothing registered, or a `src` no loader handles,
    // fires `on_error` (and logs) on Android too, instead of a silent blank.
    match image_source::classify_src(src) {
        image_source::SrcKind::Asset(id) => match cache.get(&AssetId(id)).cloned() {
            Some(CachedImage::Bitmap(image)) => set_layer_image(view, &image),
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
fn report_unloadable(view: &NSView, src: &str, why: &str) {
    backend_apple_core::apple_log(&format!(
        "[backend-macos] image: {why} ({}) — firing on_error",
        &src[..src.len().min(48)]
    ));
    if is_image_view(view) {
        report_load_failed(unsafe { &*(view as *const NSView as *const ImageView) });
    }
}

/// Decode a `data:` URI and show it, or report the failure.
fn apply_data_uri(view: &NSView, src: &str) {
    let decoded = image_source::parse_data_uri(src).and_then(|uri| {
        Some(match image_source::decode_bytes(&uri.bytes)? {
            DecodedSource::Svg(svg) => CachedImage::Svg(Rc::new(svg)),
            DecodedSource::Bitmap(bytes) => CachedImage::Bitmap(decode_image_from_bytes(&bytes)?),
        })
    });
    match decoded {
        Some(CachedImage::Bitmap(image)) => set_layer_image(view, &image),
        Some(CachedImage::Svg(svg)) => show_svg_on(view, svg),
        None => {
            backend_apple_core::apple_log(&format!(
                "[backend-macos] image: could not decode data URI ({}…) — firing on_error",
                &src[..src.len().min(48)]
            ));
            if is_image_view(view) {
                report_load_failed(unsafe { &*(view as *const NSView as *const ImageView) });
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

fn show_svg_on(view: &NSView, svg: Rc<SvgImage>) {
    if is_image_view(view) {
        show_svg(unsafe { &*(view as *const NSView as *const ImageView) }, svg);
    }
}

/// Display `svg` on `iv`: rasterize at the current bounds × backing scale
/// into `layer.contents`, record the intrinsic size as the natural size
/// (firing `on_load`), and keep the document for re-rasterizing.
fn show_svg(iv: &ImageView, svg: Rc<SvgImage>) {
    let scale = svg_scale_for(iv, &svg);
    let Some(cg) = svg.rasterize(scale).and_then(|r| apple_image::cg_image_from_raster(&r)) else {
        report_load_failed(iv);
        return;
    };
    let (w, h) = svg.intrinsic_size();
    let view: &NSView = iv;
    // `layer.contents` retains the CGImage; `cg` releases our +1 on drop.
    set_layer_contents(view, cg.as_cg_ref().0 as *mut AnyObject, CGSize::new(w, h));
    *iv.ivars().svg.borrow_mut() = Some(svg);
    iv.ivars().svg_raster_scale.set(scale);
}

/// Re-rasterize a displayed SVG if the bounds / backing scale now call for a
/// different density. Swaps `layer.contents` only — the natural size is the
/// SVG's intrinsic size and doesn't change, so no `on_load` / relayout.
fn rerasterize_svg(iv: &ImageView) {
    let svg = iv.ivars().svg.borrow().clone();
    let Some(svg) = svg else { return };
    let scale = svg_scale_for(iv, &svg);
    if scale == iv.ivars().svg_raster_scale.get() {
        return;
    }
    if let Some(cg) = svg.rasterize(scale).and_then(|r| apple_image::cg_image_from_raster(&r)) {
        iv.ivars().svg_raster_scale.set(scale);
        let layer: Retained<NSObject> = unsafe { msg_send_id![iv, layer] };
        let contents = cg.as_cg_ref().0 as *mut AnyObject;
        let _: () = unsafe { msg_send![&layer, setContents: contents] };
    }
}

/// Raster density for `svg` in `view`'s bounds and backing scale (the
/// window's, else the main screen's — a detached view still gets a raster
/// at the density it will most likely be shown at).
fn svg_scale_for(view: &NSView, svg: &SvgImage) -> f64 {
    let bounds: CGRect = unsafe { msg_send![view, bounds] };
    let window: *mut AnyObject = unsafe { msg_send![view, window] };
    let screen_scale: f64 = if !window.is_null() {
        unsafe { msg_send![window, backingScaleFactor] }
    } else {
        let screen: *mut AnyObject = unsafe { msg_send![objc2::class!(NSScreen), mainScreen] };
        if screen.is_null() { 1.0 } else { unsafe { msg_send![screen, backingScaleFactor] } }
    };
    svg.raster_scale((bounds.size.width, bounds.size.height), screen_scale)
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
            backend_apple_core::crash::abort_on_panic("image URL completion handler", || {
                // SVG bytes: hand the NSData to main, which parses +
                // rasterizes at the view's size (shared with iOS).
                if !data.is_null()
                    && image_source::looks_like_svg(unsafe { apple_image::nsdata_bytes(&*data) })
                {
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
                // yield no `NSImage`. Either way, hop the `on_error` observer
                // to main via `imageLoadFailed` (CALayer/NSView + the handler
                // touch main-thread-only state).
                let image = if data.is_null() { None } else { nsimage_from_data_ptr(data) };
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

    // Minimal 1×1 PNG (transparent). Decodes via `NSImage(data:)` to a
    // bitmap whose natural size is 1×1 — enough to drive `on_load`.
    const PNG_1X1: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x62, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    // REGRESSION: assigning a decoded bitmap fires the `on_load` observer
    // with the image's natural dimensions — both when the handler is
    // installed *before* the bitmap arrives (async-URL order) and when it
    // is installed *after* an already-decoded bitmap is set (embedded-asset
    // order, which is exactly the `create_image` → walker-install sequence).
    #[test]
    fn on_load_fires_with_natural_size_both_orders() {
        use std::cell::RefCell;
        use std::rc::Rc;
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        if unsafe { pthread_main_np() } == 0 {
            eprintln!("skipping on_load_fires_with_natural_size_both_orders: not on main thread");
            return;
        }
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        let image = decode_image_from_bytes(PNG_1X1).expect("1×1 PNG must decode");

        // Order A — handler installed BEFORE the bitmap is assigned.
        let node_a = MacosNode::View(Retained::into_super(ImageView::new(mtm)));
        let seen_a: Rc<RefCell<Vec<(f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
        {
            let s = seen_a.clone();
            install_load_handler(
                &node_a,
                Rc::new(move |ev| s.borrow_mut().push((ev.width, ev.height))),
            );
        }
        assert!(seen_a.borrow().is_empty(), "no bitmap yet → no on_load");
        let MacosNode::View(v_a) = &node_a else { unreachable!() };
        set_layer_image(v_a, &image);
        assert_eq!(*seen_a.borrow(), vec![(1.0, 1.0)], "on_load fires on assignment");

        // Order B — bitmap assigned first, handler installed after (fires now).
        let node_b = MacosNode::View(Retained::into_super(ImageView::new(mtm)));
        let MacosNode::View(v_b) = &node_b else { unreachable!() };
        set_layer_image(v_b, &image);
        let seen_b: Rc<RefCell<Vec<(f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
        {
            let s = seen_b.clone();
            install_load_handler(
                &node_b,
                Rc::new(move |ev| s.borrow_mut().push((ev.width, ev.height))),
            );
        }
        assert_eq!(
            *seen_b.borrow(),
            vec![(1.0, 1.0)],
            "installing after decode fires immediately with natural size"
        );
    }

    /// Regression (CrewForge `BrandLockup`, 2026-10-07): an
    /// `image("data:image/svg+xml,…")` drew an empty slot on Apple — the
    /// views only resolved `asset://` + `http(s)://`, and `UIImage`/`NSImage
    /// (data:)` can't decode SVG. Now the SVG is rasterized into
    /// `layer.contents`, measures at its intrinsic size, fires `on_load`, and
    /// re-rasterizes denser when the view grows. Runs the real objc2 calls,
    /// so it also pins the `CGImage` / `NSData` type encodings.
    #[test]
    fn regression_svg_data_uri_image_renders_instead_of_blank() {
        use std::cell::RefCell;
        use std::rc::Rc;
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        if unsafe { pthread_main_np() } == 0 {
            eprintln!("skipping regression_svg_data_uri_image_renders_instead_of_blank: not on main thread");
            return;
        }
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        // Percent-encoded, the way CrewForge's `svg_data_uri` builds it.
        let src = "data:image/svg+xml,%3Csvg%20xmlns%3D%22http%3A%2F%2Fwww.w3.org%2F2000%2Fsvg%22%20\
                   width%3D%2232%22%20height%3D%2216%22%3E%3Crect%20width%3D%2232%22%20\
                   height%3D%2216%22%20fill%3D%22red%22%2F%3E%3C%2Fsvg%3E";
        let node = create_image(mtm, &ImageCache::new(), src, None);
        let MacosNode::View(view) = &node else { unreachable!() };
        let iv: &ImageView = unsafe { &*(Retained::as_ptr(view) as *const ImageView) };

        let layer: Retained<NSObject> = unsafe { msg_send_id![view, layer] };
        let contents: *mut AnyObject = unsafe { msg_send![&layer, contents] };
        assert!(!contents.is_null(), "SVG data URI must put a bitmap on the layer, not leave it blank");
        let natural = iv.ivars().natural_size.get();
        assert_eq!((natural.width, natural.height), (32.0, 16.0), "measures at the SVG's intrinsic size");

        let seen: Rc<RefCell<Vec<(f32, f32)>>> = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        install_load_handler(&node, Rc::new(move |ev| s.borrow_mut().push((ev.width, ev.height))));
        assert_eq!(*seen.borrow(), vec![(32.0, 16.0)], "on_load fires with the intrinsic size");

        // Taffy sizes the view 4× larger → re-rasterized denser, same natural size.
        let before = iv.ivars().svg_raster_scale.get();
        let _: () = unsafe { msg_send![view, setFrameSize: CGSize::new(128.0, 64.0)] };
        let after = iv.ivars().svg_raster_scale.get();
        assert!(after >= before * 4.0, "re-rasterized for the larger frame: {before} -> {after}");
        assert_eq!(iv.ivars().natural_size.get().width, 32.0, "re-raster doesn't change measurement");

        // An undecodable data URI reports `on_error` instead of a silent blank.
        let bad = create_image(mtm, &ImageCache::new(), "data:image/svg+xml,%3Csvg", None);
        let fired = Rc::new(std::cell::Cell::new(false));
        let f = fired.clone();
        install_error_handler(&bad, Rc::new(move || f.set(true)));
        assert!(fired.get(), "a broken data URI fires on_error");
    }

    /// Regression (rule 7 parity with Android, 2026-10-08): an `asset://`
    /// id nothing registered, or a `src` no loader handles, left the view
    /// silently blank on Apple while Android fired `on_error`. Every native
    /// backend now routes through `image_source::classify_src` and reports
    /// both as a load error.
    #[test]
    fn regression_unloadable_image_src_was_silently_blank_on_apple() {
        extern "C" {
            fn pthread_main_np() -> std::os::raw::c_int;
        }
        if unsafe { pthread_main_np() } == 0 {
            eprintln!("skipping regression_unloadable_image_src_was_silently_blank_on_apple: not on main thread");
            return;
        }
        let mtm = unsafe { MainThreadMarker::new_unchecked() };
        for src in ["asset://424242", "ftp://example.com/a.png", ""] {
            let node = create_image(mtm, &ImageCache::new(), src, None);
            let fired = std::rc::Rc::new(std::cell::Cell::new(false));
            let f = fired.clone();
            install_error_handler(&node, std::rc::Rc::new(move || f.set(true)));
            assert!(fired.get(), "{src:?} must fire on_error");
        }
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
