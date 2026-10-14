//! `image()` source loading for the wgpu renderer — the
//! `docs/image-src.md` contract (`runtime_shared::image_src` is its
//! parser).
//!
//! Before this module the renderer resolved every `src` per frame as a
//! filesystem path: `http(s)://`, `data:` and `file://` srcs all became
//! failed disk lookups, a relative path loaded from the working directory
//! (the one platform where it "worked", so apps shipped it and broke
//! everywhere else), and a failure painted a stripe without ever firing
//! `on_error`. Loading now lives on the backend, keyed off the node, and
//! the renderer only uploads what loaded:
//!
//! | `src` (via [`image_src::classify`]) | Path |
//! |---|---|
//! | `data:` | parsed + decoded synchronously in `create_image` / `update_image_src` (web: decoded by the browser, settles on a later tick) |
//! | `asset://id`, `embed_asset!` | the registered bytes, decoded synchronously (web: as `data:`) |
//! | `asset://id`, `asset!` (Bundled) | native: `image_src::read_bundled_asset`; web: fetched from `/assets/<path>` |
//! | `asset://id`, Remote asset | fetched like `http(s)` |
//! | `http(s)://` | fetched + decoded off the render thread ([`spawn_fetch`]) |
//! | anything else | rejected: dev warning + `on_error` |
//!
//! **Threading.** A fetch runs on a worker thread on native (the `net`
//! client blocked on with [`block_on_fetch`]) and as a future on the
//! page's executor on the web. Either way the result lands in the
//! backend's `Arc<Mutex<Vec<FetchDone>>>` and the worker calls the
//! `Send + Sync` redraw hook; the next `Host::tick` drains it on the UI
//! thread ([`WgpuBackend::pump_image_loads`]) — the only place node state
//! and author handlers are touched.
//!
//! **Stale guard.** Every `src` assignment bumps the node's
//! `generation`; a completion only settles the waiters whose generation
//! still matches, so a slow response for an old `src` never replaces a
//! newer one.
//!
//! **Events.** `on_load` / `on_error` fire through
//! `schedule_microtask`, never inline: outcomes settle while the caller
//! holds the backend borrow (`create_image`, the handler installs), and
//! an author handler that re-enters the backend must not find it
//! borrowed. A handler installed after the outcome receives it on
//! install (`data:` and asset srcs settle inside `create_image`, before
//! the framework installs handlers).
//!
//! **Caching.** Successful decodes are cached by `src`, so a second node
//! (or a src swapped back) reuses the bitmap without refetching.
//! Failures are not cached: setting a failed `src` again retries.
//!
//! **Decoding.** Native decodes with the `image` crate (PNG / JPEG / GIF /
//! WebP) and parses SVG with resvg (`backend-image-source`), synchronously
//! for local bytes and on the fetch worker for remote ones. On wasm32 the
//! browser decodes ([`web`]): decoding there is asynchronous, so EVERY src
//! — `data:` and embedded assets included — settles through the same
//! completion queue a fetch uses, on a later `Host::tick` (the stale guard
//! and the waiter list apply unchanged). That is the same timing `<img>`
//! has under the DOM web backend.

#[cfg(target_arch = "wasm32")]
pub mod web;

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use runtime_shared::image_src::{self, ImageSrc};
use runtime_shared::primitives::image::ImageLoadEvent;
use runtime_shared::{AssetId, ImageErrorHandler, ImageLoadHandler};

use crate::node::{NodeData, NodeKind, WgpuNode};
use crate::WgpuBackend;

/// How long an image fetch may take before it fails with `on_error`. A
/// stalled server would otherwise pin a worker thread (native) and leave
/// the image pending forever. Generous because images are large and
/// mobile links are slow; the point is "eventually fails", not "fast".
const IMAGE_FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// Monotonic id for each decoded image. The renderer keys its GPU
/// textures on it (not on `src`), so a re-registered asset or a re-decoded
/// src never samples a stale texture uploaded for the old bytes.
static NEXT_IMAGE_ID: AtomicU64 = AtomicU64::new(1);

fn next_image_id() -> u64 {
    NEXT_IMAGE_ID.fetch_add(1, Ordering::Relaxed)
}

/// A decoded image, ready for upload.
pub struct DecodedImage {
    /// Unique per decode — see [`NEXT_IMAGE_ID`].
    pub id: u64,
    pub kind: DecodedKind,
}

pub enum DecodedKind {
    /// Straight-alpha sRGB RGBA8 (what the `image` crate decodes to and
    /// the image pipeline's `ALPHA_BLENDING` expects).
    Raster(image::RgbaImage),
    /// Rasterized by the renderer at the size it is displayed.
    Svg(SvgDoc),
}

/// A parsed SVG: resvg's tree on native, a browser-decoded image element
/// on the web. Both expose `intrinsic_size()`; [`rasterize_svg`] draws
/// either at a scale.
#[cfg(not(target_arch = "wasm32"))]
pub type SvgDoc = backend_image_source::SvgImage;
#[cfg(target_arch = "wasm32")]
pub type SvgDoc = web::BrowserSvg;

/// Rasterize `svg` at `scale` pixels per intrinsic point, as straight-alpha
/// RGBA8 (the image pipeline blends straight alpha,
/// `BlendState::ALPHA_BLENDING`). `None` for a degenerate document.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn rasterize_svg(svg: &SvgDoc, scale: f64) -> Option<image::RgbaImage> {
    let raster = svg.rasterize(scale)?;
    let mut rgba = image::RgbaImage::from_raw(raster.width_px, raster.height_px, raster.rgba_premultiplied)?;
    // resvg rasterizes premultiplied.
    demultiply(&mut rgba);
    Some(rgba)
}

/// Web: the browser canvas already reads back straight alpha
/// (`getImageData`).
#[cfg(target_arch = "wasm32")]
pub(crate) fn rasterize_svg(svg: &SvgDoc, scale: f64) -> Option<image::RgbaImage> {
    svg.rasterize(scale)
}

/// Premultiplied → straight alpha, in place.
#[cfg(not(target_arch = "wasm32"))]
fn demultiply(rgba: &mut image::RgbaImage) {
    for px in rgba.pixels_mut() {
        let a = px.0[3] as u32;
        if a != 0 && a != 255 {
            for c in &mut px.0[..3] {
                *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

impl DecodedImage {
    /// Natural size: pixel dimensions for a bitmap, the document's
    /// intrinsic size for an SVG. What `on_load` reports and what an
    /// unsized image lays out at.
    pub fn natural_size(&self) -> (f32, f32) {
        match &self.kind {
            DecodedKind::Raster(img) => (img.width() as f32, img.height() as f32),
            DecodedKind::Svg(svg) => {
                let (w, h) = svg.intrinsic_size();
                (w as f32, h as f32)
            }
        }
    }
}

/// Decode image bytes: SVG when they sniff as one, otherwise whatever the
/// `image` crate recognizes (PNG / JPEG / GIF first frame / WebP). Native
/// only — on the web, see [`web::decode`].
#[cfg(not(target_arch = "wasm32"))]
pub fn decode(bytes: &[u8]) -> Result<DecodedImage, String> {
    let kind = if backend_image_source::looks_like_svg(bytes) {
        DecodedKind::Svg(
            backend_image_source::SvgImage::parse(bytes).ok_or("invalid SVG document")?,
        )
    } else {
        DecodedKind::Raster(image::load_from_memory(bytes).map_err(|e| e.to_string())?.to_rgba8())
    };
    Ok(DecodedImage { id: next_image_id(), kind })
}

/// How a node's current `src` ended up.
#[derive(Clone)]
pub enum ImageOutcome {
    Loaded(Rc<DecodedImage>),
    Failed,
}

/// Per-node load state, carried on `NodeKind::Image`.
#[derive(Default)]
pub struct ImageLoadState {
    /// Bumped on every `src` assignment — the stale guard.
    pub generation: u64,
    /// `None` while a fetch is pending (or before the first load).
    pub outcome: Option<ImageOutcome>,
    pub on_load: Option<ImageLoadHandler>,
    pub on_error: Option<ImageErrorHandler>,
    /// Whether the current outcome was already queued for the installed
    /// `on_load` / `on_error`. Exactly one delivery per outcome per
    /// handler: a replay on install and the live settle can't both fire.
    /// Reset by a new src and by installing a new handler.
    load_delivered: bool,
    error_delivered: bool,
    /// Natural size once loaded; read by the node's Taffy measure fn so
    /// an unsized image lays out at its natural size, like `<img>`,
    /// `UIImageView` and `NSImageView`.
    pub natural: Rc<Cell<Option<(f32, f32)>>>,
}

/// One finished fetch, handed from the worker to the UI thread.
pub(crate) struct FetchDone {
    /// The `src` (cache key) the fetch was for — not the fetched URL,
    /// which differs for `asset://` srcs.
    key: String,
    result: Result<DecodedImage, String>,
}

/// Backend-owned loader state (see the module docs).
#[derive(Default)]
pub struct ImageLoader {
    /// Successful decodes by `src`.
    cache: HashMap<String, Rc<DecodedImage>>,
    /// `src`s with a fetch in flight — a second node asking for the same
    /// src waits on the first fetch instead of starting another.
    in_flight: HashSet<String>,
    /// Nodes waiting on a fetch, with the generation they asked at.
    waiters: HashMap<String, Vec<(Weak<std::cell::RefCell<NodeData>>, u64)>>,
    completions: Arc<Mutex<Vec<FetchDone>>>,
    /// `asset!` image paths by id (bytes not in the binary).
    pub(crate) bundled_assets: HashMap<AssetId, &'static str>,
    /// Remote image asset URLs by id.
    pub(crate) remote_assets: HashMap<AssetId, &'static str>,
}

impl ImageLoader {
    /// Drop everything cached for `src` (asset hot-reload / unregister).
    pub(crate) fn evict(&mut self, src: &str) {
        self.cache.remove(src);
    }

    /// `true` while any fetch is in flight — tests poll on it.
    pub fn has_pending_fetches(&self) -> bool {
        !self.in_flight.is_empty()
    }
}

/// The asynchronous work behind a pending `src`.
enum LoadJob {
    /// GET this URL, then decode the body.
    Fetch(String),
    /// Web only: decode local bytes with the browser (asynchronous there).
    #[cfg(target_arch = "wasm32")]
    Decode(Vec<u8>),
}

/// What resolving a `src` produced right now.
enum Resolution {
    Ready(Rc<DecodedImage>),
    Failed,
    /// A fetch is in flight; the node was registered as a waiter.
    Pending,
}

impl WgpuBackend {
    /// Start loading `node`'s current `src`: bump its generation, clear
    /// the previous outcome, and settle synchronously when the source is
    /// local (`data:`, embedded asset, bundled file on native) or
    /// rejected. Remote sources settle later via [`Self::pump_image_loads`].
    pub(crate) fn begin_image_load(&mut self, node: &WgpuNode) {
        let (src, generation) = {
            let mut n = node.borrow_mut();
            let NodeKind::Image { src, load, .. } = &mut n.kind else { return };
            load.generation += 1;
            load.outcome = None;
            load.load_delivered = false;
            load.error_delivered = false;
            load.natural.set(None);
            (src.clone(), load.generation)
        };
        let layout = node.borrow().layout;
        self.layout.mark_dirty(layout);
        match self.resolve_image_src(&src, node, generation) {
            Resolution::Ready(img) => settle(node, generation, ImageOutcome::Loaded(img)),
            Resolution::Failed => settle(node, generation, ImageOutcome::Failed),
            Resolution::Pending => {}
        }
    }

    fn resolve_image_src(&mut self, src: &str, node: &WgpuNode, generation: u64) -> Resolution {
        if let Some(img) = self.images.cache.get(src) {
            return Resolution::Ready(img.clone());
        }
        // The bytes of a local source (`data:`, embedded or — on native —
        // bundled asset); remote sources return early as a pending fetch.
        let bytes: Result<std::borrow::Cow<'_, [u8]>, String> = match image_src::classify(src) {
            ImageSrc::Rejected(reason) => {
                image_src::warn_rejected(src, reason);
                return Resolution::Failed;
            }
            ImageSrc::Data(uri) => match image_src::parse_data_uri(uri) {
                Some(data) => Ok(data.bytes.into()),
                None => Err("malformed data: URI".to_string()),
            },
            ImageSrc::Remote(url) => {
                return self.fetch_for(src, LoadJob::Fetch(url.trim().to_string()), node, generation);
            }
            ImageSrc::Asset(id) => {
                let id = AssetId(id);
                if let Some(bytes) = self.image_asset_bytes.get(&id) {
                    Ok(bytes.as_slice().into())
                } else if let Some(path) = self.images.bundled_assets.get(&id).copied() {
                    #[cfg(not(target_arch = "wasm32"))]
                    {
                        image_src::read_bundled_asset(path)
                            .map(Into::into)
                            .ok_or_else(|| format!("bundled asset {path:?} not found"))
                    }
                    #[cfg(target_arch = "wasm32")]
                    {
                        return self.fetch_for(src, LoadJob::Fetch(format!("/assets/{path}")), node, generation);
                    }
                } else if let Some(url) = self.images.remote_assets.get(&id).copied() {
                    return self.fetch_for(src, LoadJob::Fetch(url.to_string()), node, generation);
                } else {
                    Err(format!("image asset {} not found", id.0))
                }
            }
        };
        // Native decodes local bytes right here; the browser decodes
        // asynchronously, so on the web they settle like a fetch.
        #[cfg(not(target_arch = "wasm32"))]
        let decoded = bytes.and_then(|b| decode(&b));
        #[cfg(target_arch = "wasm32")]
        let decoded: Result<DecodedImage, String> = match bytes {
            Ok(b) => {
                let b = b.into_owned();
                return self.fetch_for(src, LoadJob::Decode(b), node, generation);
            }
            Err(e) => Err(e),
        };
        match decoded {
            Ok(img) => {
                let img = Rc::new(img);
                self.images.cache.insert(src.to_string(), img.clone());
                Resolution::Ready(img)
            }
            Err(e) => {
                log::warn!("image(): {:?} failed to load: {e}", truncate_src(src));
                Resolution::Failed
            }
        }
    }

    /// Register `node` as waiting on `src`, starting `job` unless a load
    /// for `src` is already in flight.
    fn fetch_for(&mut self, src: &str, job: LoadJob, node: &WgpuNode, generation: u64) -> Resolution {
        self.images
            .waiters
            .entry(src.to_string())
            .or_default()
            .push((Rc::downgrade(node), generation));
        if self.images.in_flight.insert(src.to_string()) {
            let sink = self.images.completions.clone();
            match job {
                LoadJob::Fetch(url) => spawn_fetch(url, src.to_string(), sink),
                #[cfg(target_arch = "wasm32")]
                LoadJob::Decode(bytes) => spawn_decode(bytes, src.to_string(), sink),
            }
        }
        Resolution::Pending
    }

    /// Drain finished fetches (UI thread). Settles every still-current
    /// waiter and returns `true` if anything finished — the caller then
    /// needs another frame. Called from `Host::tick`.
    pub fn pump_image_loads(&mut self) -> bool {
        let done: Vec<FetchDone> = std::mem::take(&mut *self.images.completions.lock().unwrap());
        if done.is_empty() {
            return false;
        }
        for FetchDone { key, result } in done {
            self.images.in_flight.remove(&key);
            let outcome = match result {
                Ok(img) => {
                    let img = Rc::new(img);
                    self.images.cache.insert(key.clone(), img.clone());
                    ImageOutcome::Loaded(img)
                }
                Err(e) => {
                    log::warn!("image(): {:?} failed to load: {e}", truncate_src(&key));
                    ImageOutcome::Failed
                }
            };
            for (weak, generation) in self.images.waiters.remove(&key).unwrap_or_default() {
                if let Some(node) = weak.upgrade() {
                    let layout = node.borrow().layout;
                    self.layout.mark_dirty(layout);
                    settle(&node, generation, outcome.clone());
                }
            }
        }
        crate::scheduler::request_redraw();
        true
    }

    /// Whether any image fetch is still in flight.
    pub fn has_pending_image_loads(&self) -> bool {
        self.images.has_pending_fetches()
    }

    pub(crate) fn install_image_load_handler_impl(&mut self, node: &WgpuNode, handler: ImageLoadHandler) {
        if let NodeKind::Image { load, .. } = &mut node.borrow_mut().kind {
            load.on_load = Some(handler);
            load.load_delivered = false;
        } else {
            return;
        }
        // Replay: a local src settled inside `create_image`, before this
        // install.
        deliver(node, Deliver::Load);
    }

    pub(crate) fn install_image_error_handler_impl(&mut self, node: &WgpuNode, handler: ImageErrorHandler) {
        if let NodeKind::Image { load, .. } = &mut node.borrow_mut().kind {
            load.on_error = Some(handler);
            load.error_delivered = false;
        } else {
            return;
        }
        deliver(node, Deliver::Error);
    }
}

/// Record `outcome` on `node` if `generation` is still its current one,
/// then deliver it to whichever handler is installed.
fn settle(node: &WgpuNode, generation: u64, outcome: ImageOutcome) {
    {
        let mut n = node.borrow_mut();
        let NodeKind::Image { load, .. } = &mut n.kind else { return };
        if load.generation != generation {
            return; // superseded by a newer src
        }
        load.natural.set(match &outcome {
            ImageOutcome::Loaded(img) => Some(img.natural_size()),
            ImageOutcome::Failed => None,
        });
        load.outcome = Some(outcome);
    }
    crate::scheduler::request_redraw();
    deliver(node, Deliver::Load);
    deliver(node, Deliver::Error);
}

#[derive(Clone, Copy)]
enum Deliver {
    Load,
    Error,
}

/// Queue `which` handler for the node's current outcome, if both exist
/// and match. Runs on a microtask (module docs, "Events") and re-checks
/// the generation there, so an event for a src replaced in between is
/// dropped.
fn deliver(node: &WgpuNode, which: Deliver) {
    let mut n = node.borrow_mut();
    let NodeKind::Image { load, .. } = &mut n.kind else { return };
    let generation = load.generation;
    let fire: Box<dyn FnOnce()> = match (which, &load.outcome, &load.on_load, &load.on_error) {
        (Deliver::Load, Some(ImageOutcome::Loaded(img)), Some(h), _) if !load.load_delivered => {
            let (width, height) = img.natural_size();
            let h = h.clone();
            load.load_delivered = true;
            Box::new(move || h(&ImageLoadEvent { width, height }))
        }
        (Deliver::Error, Some(ImageOutcome::Failed), _, Some(h)) if !load.error_delivered => {
            let h = h.clone();
            load.error_delivered = true;
            Box::new(move || h())
        }
        _ => return,
    };
    drop(n);
    let weak = Rc::downgrade(node);
    runtime_shared::scheduling::schedule_microtask(move || {
        let current = weak.upgrade().is_some_and(|node| {
            matches!(&node.borrow().kind, NodeKind::Image { load, .. } if load.generation == generation)
        });
        if current {
            fire();
        }
    });
}

/// The Taffy measure fn for an image node: its natural size once loaded
/// (each axis yielding to a size the layout already fixed — the same
/// shape as the macOS/iOS `intrinsicContentSize` measurer), and
/// `IMAGE_DEFAULT_SIZE` square until then so a pending or failed image
/// keeps a visible slot.
pub(crate) fn image_measure_fn(natural: Rc<Cell<Option<(f32, f32)>>>) -> runtime_layout::MeasureFn {
    Rc::new(move |known, _available| {
        let (w, h) = natural
            .get()
            .unwrap_or((crate::node::IMAGE_DEFAULT_SIZE, crate::node::IMAGE_DEFAULT_SIZE));
        runtime_layout::Size {
            width: known.width.unwrap_or(w),
            height: known.height.unwrap_or(h),
        }
    })
}

/// A `data:` URI can be megabytes; the log needs only its head.
fn truncate_src(src: &str) -> &str {
    const MAX: usize = 120;
    match src.char_indices().nth(MAX) {
        Some((i, _)) => &src[..i],
        None => src,
    }
}

// ---------------------------------------------------------------------------
// Fetching
// ---------------------------------------------------------------------------

/// GET `url` with the framework `net` client; non-2xx is an error.
async fn fetch_bytes(url: String) -> Result<Vec<u8>, String> {
    let resp = net::Client::new()
        .get(&url)
        .timeout(IMAGE_FETCH_TIMEOUT)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    resp.bytes().await.map_err(|e| e.to_string())
}

/// Fetch + decode `url` off the UI thread, then hand the result to the
/// loader's completion queue under `key` and wake the event loop.
#[cfg(not(target_arch = "wasm32"))]
fn spawn_fetch(url: String, key: String, sink: Arc<Mutex<Vec<FetchDone>>>) {
    std::thread::Builder::new()
        .name("image-fetch".into())
        .spawn(move || {
            // Decode on the worker too — a large JPEG decode would
            // otherwise stall a frame.
            let result = block_on_fetch(fetch_bytes(url)).and_then(|b| decode(&b));
            sink.lock().unwrap().push(FetchDone { key, result });
            crate::scheduler::request_redraw();
        })
        .expect("spawn image-fetch thread");
}

#[cfg(target_arch = "wasm32")]
fn spawn_fetch(url: String, key: String, sink: Arc<Mutex<Vec<FetchDone>>>) {
    runtime_shared::driver::spawn_async(async move {
        let result = match fetch_bytes(url).await {
            Ok(bytes) => web::decode(bytes).await,
            Err(e) => Err(e),
        };
        sink.lock().unwrap().push(FetchDone { key, result });
        crate::scheduler::request_redraw();
    });
}

/// Decode local bytes with the browser, then hand the result to the
/// completion queue like a finished fetch.
#[cfg(target_arch = "wasm32")]
fn spawn_decode(bytes: Vec<u8>, key: String, sink: Arc<Mutex<Vec<FetchDone>>>) {
    runtime_shared::driver::spawn_async(async move {
        let result = web::decode(bytes).await;
        sink.lock().unwrap().push(FetchDone { key, result });
        crate::scheduler::request_redraw();
    });
}

/// Drive the fetch future to completion on the calling (worker) thread.
/// Every native `net` transport wakes its future from its own I/O thread
/// (reqwest's process-wide runtime on Linux/Windows, the NSURLSession
/// completion on Apple, the JNI worker on Android) and needs no ambient
/// runtime, so a park/unpark loop is all the executor it needs.
#[cfg(not(target_arch = "wasm32"))]
fn block_on_fetch<F: std::future::Future>(fut: F) -> F::Output {
    use std::task::{Context, Poll, Wake, Waker};
    struct Unpark(std::thread::Thread);
    impl Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        match fut.as_mut().poll(&mut cx) {
            Poll::Ready(v) => return v,
            Poll::Pending => std::thread::park(),
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    //! Lib unit tests run with no scheduler installed, so
    //! `schedule_microtask` runs inline: an event fires during the call
    //! that delivers it, which is what lets these assert synchronously.

    use super::*;
    use runtime_shared::image_src::vectors::{self, Expect};
    use std::cell::RefCell;
    use std::io::{Read, Write};

    #[derive(Debug, Clone, PartialEq)]
    enum Event {
        Load(f32, f32),
        Error,
    }

    fn make_backend() -> WgpuBackend {
        let text = Rc::new(RefCell::new(crate::text::TextStore::new()));
        let fs = Rc::new(RefCell::new(glyphon::FontSystem::new()));
        WgpuBackend::new(
            text,
            fs,
            runtime_shared::ColorScheme::Light,
            Rc::new(crate::NativeSkin::new(runtime_shared::Platform::MacOs)),
        )
    }

    fn install(b: &mut WgpuBackend, node: &WgpuNode) -> Rc<RefCell<Vec<Event>>> {
        let log = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        b.install_image_load_handler_impl(
            node,
            Rc::new(move |ev: &ImageLoadEvent| l.borrow_mut().push(Event::Load(ev.width, ev.height))),
        );
        let l = log.clone();
        b.install_image_error_handler_impl(node, Rc::new(move || l.borrow_mut().push(Event::Error)));
        log
    }

    fn image(b: &mut WgpuBackend, src: &str) -> (WgpuNode, Rc<RefCell<Vec<Event>>>) {
        let node = b.create_image_impl(src, None, &Default::default());
        let log = install(b, &node);
        (node, log)
    }

    fn outcome(node: &WgpuNode) -> Option<ImageOutcome> {
        match &node.borrow().kind {
            NodeKind::Image { load, .. } => load.outcome.clone(),
            _ => unreachable!(),
        }
    }

    /// Pump until no fetch is in flight (or panic after 10 s).
    fn settle_fetches(b: &mut WgpuBackend) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while b.has_pending_image_loads() {
            b.pump_image_loads();
            assert!(std::time::Instant::now() < deadline, "image fetch never completed");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// A one-request-per-connection HTTP/1.1 server on 127.0.0.1. Each
    /// accepted request is answered with `respond(path)` →
    /// `(status line, body)`. Returns the base URL.
    fn serve(respond: impl Fn(&str) -> (&'static str, Vec<u8>) + Send + 'static) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => buf.extend_from_slice(&chunk[..n]),
                    }
                }
                let req = String::from_utf8_lossy(&buf);
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let (status, body) = respond(&path);
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        base
    }

    fn png_1x1() -> Vec<u8> {
        image_src::parse_data_uri(vectors::PNG_1X1_BASE64).unwrap().bytes
    }

    /// Every synchronous conformance vector through the real
    /// `create_image` path: `Loads` → `on_load` with the natural size,
    /// `Errors` → `on_error`.
    #[test]
    fn sync_vectors_follow_the_contract() {
        let mut b = make_backend();
        for (src, expect) in vectors::SYNC {
            let (_node, log) = image(&mut b, src);
            let want = match expect {
                Expect::Loads { width, height } => Event::Load(*width as f32, *height as f32),
                Expect::Errors => Event::Error,
            };
            assert_eq!(*log.borrow(), vec![want], "{src:?}");
        }
    }

    /// The renderer used to read any non-`asset://` src from disk —
    /// absolute, or relative to the cwd / exe dir / workspace root — so
    /// `/attachments/x` "worked" on the wgpu renderer only. A path is
    /// now rejected without touching the filesystem.
    #[test]
    fn regression_relative_path_no_longer_read_from_disk() {
        let mut b = make_backend();
        // A real, decodable PNG at both an absolute and a cwd-relative
        // path — exactly what the old resolver would have loaded.
        let abs = std::env::temp_dir().join(format!("wgpu-img-src-{}.png", std::process::id()));
        std::fs::write(&abs, png_1x1()).unwrap();
        let rel = format!("wgpu-img-src-{}.png", std::process::id());
        std::fs::write(&rel, png_1x1()).unwrap();
        struct Cleanup(std::path::PathBuf, String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_file(&self.0);
                let _ = std::fs::remove_file(&self.1);
            }
        }
        let _cleanup = Cleanup(abs.clone(), rel.clone());

        for src in [abs.to_str().unwrap().to_string(), rel.clone(), format!("./{rel}")] {
            let (node, log) = image(&mut b, &src);
            assert!(matches!(outcome(&node), Some(ImageOutcome::Failed)), "{src:?} must be rejected");
            assert_eq!(*log.borrow(), vec![Event::Error], "{src:?}");
        }
    }

    /// `http(s)` srcs used to become filesystem lookups (always failing).
    /// They are fetched off the UI thread, settle on the next pump, and
    /// report the natural size.
    #[test]
    fn regression_http_src_loads_instead_of_disk_lookup() {
        let base = serve(|path| match path {
            "/ok.png" => ("200 OK", png_1x1()),
            _ => ("404 Not Found", b"nope".to_vec()),
        });
        let mut b = make_backend();
        let (node, log) = image(&mut b, &format!("{base}/ok.png"));
        assert!(outcome(&node).is_none(), "remote load is asynchronous");
        assert!(log.borrow().is_empty());
        settle_fetches(&mut b);
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)]);

        // Non-2xx is an error, not a decode of the error page.
        let (node, log) = image(&mut b, &format!("{base}/missing.png"));
        settle_fetches(&mut b);
        assert!(matches!(outcome(&node), Some(ImageOutcome::Failed)));
        assert_eq!(*log.borrow(), vec![Event::Error]);

        // A second image with the loaded src reuses the cached decode:
        // it settles synchronously, no fetch.
        let (_node, log) = image(&mut b, &format!("{base}/ok.png"));
        assert!(!b.has_pending_image_loads());
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)]);
    }

    /// A slow response for a replaced src must not overwrite the newer
    /// src's outcome or fire its handlers.
    #[test]
    fn stale_remote_response_never_replaces_newer_src() {
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = Mutex::new(release_rx);
        let base = serve(move |_| {
            let _ = release_rx.lock().unwrap().recv();
            ("200 OK", png_1x1())
        });
        let mut b = make_backend();
        let (node, log) = image(&mut b, &format!("{base}/slow.png"));
        b.update_image_src_impl(&node, vectors::SVG_2X1_PERCENT);
        assert_eq!(*log.borrow(), vec![Event::Load(2.0, 1.0)]);
        release_tx.send(()).unwrap();
        settle_fetches(&mut b);
        assert_eq!(*log.borrow(), vec![Event::Load(2.0, 1.0)], "stale response fired a handler");
        match outcome(&node) {
            Some(ImageOutcome::Loaded(img)) => assert_eq!(img.natural_size(), (2.0, 1.0)),
            _ => panic!("newer src's outcome was replaced"),
        }
    }

    /// The renderer painted a placeholder for a broken src but never told
    /// the app: `on_error` never fired. It now fires — for a handler
    /// installed after the failure (replay) and for a src swapped to a
    /// broken one while the handler is installed.
    #[test]
    fn regression_image_failure_fires_on_error() {
        let mut b = make_backend();
        // Installed after the (synchronous) failure: replayed.
        let (node, log) = image(&mut b, "data:image/png;base64,@@not-base64@@");
        assert_eq!(*log.borrow(), vec![Event::Error]);

        // Undecodable bytes behind a well-formed data: URI.
        let (_n, log2) = image(&mut b, "data:image/png;base64,AAAA");
        assert_eq!(*log2.borrow(), vec![Event::Error]);

        // Swap to a good src, then back to a rejected one.
        b.update_image_src_impl(&node, vectors::PNG_1X1_BASE64);
        b.update_image_src_impl(&node, "file:///tmp/x.png");
        assert_eq!(
            *log.borrow(),
            vec![Event::Error, Event::Load(1.0, 1.0), Event::Error]
        );
    }

    /// Failures aren't cached: a src that failed loads afresh when it is
    /// set again after another src. Re-applying the current src (failed
    /// or loaded) is a no-op.
    #[test]
    fn failed_src_set_again_retries() {
        let ok = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ok2 = ok.clone();
        let base = serve(move |_| {
            if ok2.load(Ordering::SeqCst) {
                ("200 OK", png_1x1())
            } else {
                ("503 Service Unavailable", Vec::new())
            }
        });
        let src = format!("{base}/flaky.png");
        let mut b = make_backend();
        let (node, log) = image(&mut b, &src);
        settle_fetches(&mut b);
        assert_eq!(*log.borrow(), vec![Event::Error]);
        ok.store(true, Ordering::SeqCst);
        // Same src again: no-op, even though it failed.
        b.update_image_src_impl(&node, &src);
        assert!(!b.has_pending_image_loads());
        assert_eq!(*log.borrow(), vec![Event::Error]);
        // Away and back: refetched.
        b.update_image_src_impl(&node, vectors::PNG_1X1_BASE64);
        b.update_image_src_impl(&node, &src);
        settle_fetches(&mut b);
        assert_eq!(
            *log.borrow(),
            vec![Event::Error, Event::Load(1.0, 1.0), Event::Load(1.0, 1.0)]
        );
        // Unchanged + loaded: no reload, no second event.
        b.update_image_src_impl(&node, &src);
        assert!(!b.has_pending_image_loads());
        assert_eq!(log.borrow().len(), 3);
    }

    /// Mount runs `create_image` then `update_image_src` with the same
    /// value, and installs handlers in between: one event per outcome,
    /// for loads and failures alike; each distinct failing src errors
    /// once.
    #[test]
    fn same_src_reapplied_fires_once() {
        let mut b = make_backend();
        for (src, want) in [
            (vectors::PNG_1X1_BASE64, Event::Load(1.0, 1.0)),
            ("/attachments/abc", Event::Error),
        ] {
            let (node, log) = image(&mut b, src);
            b.update_image_src_impl(&node, src);
            assert_eq!(*log.borrow(), vec![want.clone()], "{src:?}");
        }
        let (node, log) = image(&mut b, "/a.png");
        b.update_image_src_impl(&node, "/b.png");
        b.update_image_src_impl(&node, "/b.png");
        assert_eq!(*log.borrow(), vec![Event::Error, Event::Error]);
    }

    /// With a real (queueing) scheduler the live settle and an install
    /// replay are both pending at once; only one may fire. A handler
    /// installed after delivery gets its own single replay.
    #[test]
    fn on_load_fires_once_when_settle_and_install_race() {
        let mut b = make_backend();
        let (node, log) = image(&mut b, vectors::PNG_1X1_BASE64);
        // Settle already delivered to the installed handler; a second
        // delivery attempt for the same outcome must not fire it.
        deliver(&node, Deliver::Load);
        deliver(&node, Deliver::Error);
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)]);
        // A replacement handler: exactly one replay.
        let log2 = install(&mut b, &node);
        deliver(&node, Deliver::Load);
        assert_eq!(*log2.borrow(), vec![Event::Load(1.0, 1.0)]);
        assert_eq!(log.borrow().len(), 1);
    }

    /// `image_asset(..)`: embedded bytes decode; a bundled `asset!` path
    /// reads `<cwd>/assets/<path>`; an unregistered id errors.
    #[test]
    fn asset_srcs_load_embedded_and_bundled() {
        use runtime_shared::{AssetSource, AssetTag};
        static PNG: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
        let png: &'static [u8] = PNG.get_or_init(png_1x1);
        let mut b = make_backend();
        b.register_asset_impl(AssetId(7), AssetTag::Image, &AssetSource::Embedded { bytes: png, extension: "png" });
        let (_n, log) = image(&mut b, "asset://7");
        assert_eq!(*log.borrow(), vec![Event::Load(1.0, 1.0)]);

        // cargo runs tests with the crate dir as cwd, which has an
        // `assets/` dir; the path is the `asset!` argument (no prefix).
        let rel = format!("wgpu-img-asset-{}.svg", std::process::id());
        let on_disk = std::path::Path::new("assets").join(&rel);
        std::fs::write(&on_disk, image_src::parse_data_uri(vectors::SVG_2X1_PERCENT).unwrap().bytes).unwrap();
        let leaked: &'static str = Box::leak(rel.into_boxed_str());
        b.register_asset_impl(AssetId(8), AssetTag::Image, &AssetSource::Bundled { path: leaked });
        let (_n, log) = image(&mut b, "asset://8");
        let _ = std::fs::remove_file(&on_disk);
        assert_eq!(*log.borrow(), vec![Event::Load(2.0, 1.0)]);

        let (_n, log) = image(&mut b, "asset://9");
        assert_eq!(*log.borrow(), vec![Event::Error]);
    }

    /// GIF (first frame) and WebP decode — the formats the contract adds
    /// over the old PNG+JPEG build.
    #[test]
    fn gif_and_webp_decode() {
        let rgba = image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 0, 0, 255]));
        for fmt in [image::ImageFormat::Gif, image::ImageFormat::WebP] {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::DynamicImage::ImageRgba8(rgba.clone()).write_to(&mut bytes, fmt).unwrap();
            let img = decode(bytes.get_ref()).unwrap_or_else(|e| panic!("{fmt:?}: {e}"));
            assert_eq!(img.natural_size(), (3.0, 2.0), "{fmt:?}");
        }
    }

    /// An unsized image lays out at its natural size once loaded, and at
    /// the default slot size until then; a size the layout fixed wins.
    #[test]
    fn loaded_image_measures_at_natural_size() {
        use runtime_layout::{AvailableSpace, Size};
        let mut b = make_backend();
        let (node, _log) = image(&mut b, vectors::SVG_2X1_PERCENT);
        let natural = match &node.borrow().kind {
            NodeKind::Image { load, .. } => load.natural.clone(),
            _ => unreachable!(),
        };
        let measure = image_measure_fn(natural.clone());
        let avail = Size { width: AvailableSpace::MaxContent, height: AvailableSpace::MaxContent };
        let got = measure(Size { width: None, height: None }, avail);
        assert_eq!((got.width, got.height), (2.0, 1.0));
        let got = measure(Size { width: Some(40.0), height: None }, avail);
        assert_eq!((got.width, got.height), (40.0, 1.0));
        natural.set(None);
        let got = measure(Size { width: None, height: None }, avail);
        assert_eq!((got.width, got.height), (crate::node::IMAGE_DEFAULT_SIZE, crate::node::IMAGE_DEFAULT_SIZE));
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod web_tests {
    //! The browser decode path (`image_load::web`), in a real browser:
    //! `cargo test -p render-wgpu --target wasm32-unknown-unknown --lib`
    //! with `CHROMEDRIVER` set. The native `tests` module can't cover it —
    //! the decoders it exercises (`image`, resvg) aren't linked on wasm32.

    use super::*;
    use runtime_shared::image_src::vectors::{self, Expect};
    use std::cell::RefCell;
    use std::future::Future;
    use std::pin::Pin;
    use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};
    use web_glue::js::Function;
    use web_glue::{JsFuture, JsValue};

    wasm_bindgen_test_configure!(run_in_browser);

    /// `spawn_async` on wasm32 needs an executor (`backend-web` installs
    /// one in a real app); this is the same shape, without backend-web.
    struct GlueExecutor;
    impl runtime_shared::driver::AsyncExecutor for GlueExecutor {
        fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
            web_glue::spawn_local(future);
        }
    }

    fn install_executor() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| runtime_shared::driver::install_async_executor(Box::new(GlueExecutor)));
    }

    async fn sleep_ms(ms: u32) {
        let f = Function::new_no_args(&format!("return new Promise(r => setTimeout(r, {ms}))"));
        JsFuture::new(&f.call0(&JsValue::undefined()).unwrap()).await.unwrap();
    }

    fn bytes(uri: &str) -> Vec<u8> {
        image_src::parse_data_uri(uri).unwrap().bytes
    }

    fn rgba_at(img: &image::RgbaImage, x: u32, y: u32) -> [u8; 4] {
        img.get_pixel(x, y).0
    }

    /// PNG (and GIF / WebP — formats the native build gets from `image`
    /// features) decode through `createImageBitmap` to straight-alpha
    /// RGBA at the natural size.
    #[wasm_bindgen_test]
    async fn browser_decodes_rasters_to_rgba() {
        let png = web::decode(bytes(vectors::PNG_1X1_BASE64)).await.expect("png decodes");
        assert_eq!(png.natural_size(), (1.0, 1.0));
        let DecodedKind::Raster(rgba) = &png.kind else { panic!("png must be a raster") };
        assert_eq!(rgba_at(rgba, 0, 0), [255, 0, 0, 255]);

        // 1×1 GIF and 1×1 lossless WebP.
        for uri in [
            "data:image/gif;base64,R0lGODlhAQABAIAAAP8AAAAAACH5BAAAAAAALAAAAAABAAEAAAICRAEAOw==",
            "data:image/webp;base64,UklGRhoAAABXRUJQVlA4TA0AAAAvAAAAEAcQERGIiP4HAA==",
        ] {
            let img = web::decode(bytes(uri)).await.unwrap_or_else(|e| panic!("{uri}: {e}"));
            assert_eq!(img.natural_size(), (1.0, 1.0), "{uri}");
            assert!(matches!(img.kind, DecodedKind::Raster(_)));
        }
    }

    /// SVG keeps its intrinsic size (what `on_load` reports) and
    /// re-rasterizes at any scale — the renderer asks for displayed size ×
    /// devicePixelRatio, never a stretched 1× bitmap.
    #[wasm_bindgen_test]
    async fn browser_svg_reports_intrinsic_size_and_rasterizes_at_scale() {
        let img = web::decode(bytes(vectors::SVG_2X1_PERCENT)).await.expect("svg decodes");
        assert_eq!(img.natural_size(), (2.0, 1.0));
        let DecodedKind::Svg(svg) = &img.kind else { panic!("svg must stay vector") };
        let r = rasterize_svg(svg, 8.0).expect("rasterizes");
        assert_eq!(r.dimensions(), (16, 8));
        assert_eq!(rgba_at(&r, 8, 4), [255, 0, 0, 255]);
        let r = rasterize_svg(svg, 1.5).expect("rasterizes");
        assert_eq!(r.dimensions(), (3, 2), "rounded like the native resvg path");
    }

    /// The intrinsic size follows usvg (the native path), not the browser's
    /// `<img>` default: a `viewBox`-only SVG is its `viewBox` size (Chrome
    /// alone would say 300×150), a lone axis follows the `viewBox` aspect,
    /// and percentages fall back to the `viewBox`.
    #[wasm_bindgen_test]
    async fn browser_svg_intrinsic_size_matches_usvg_rules() {
        let cases: [(&str, (f32, f32)); 4] = [
            (r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 16"><rect width="32" height="16"/></svg>"#, (32.0, 16.0)),
            (r#"<svg xmlns="http://www.w3.org/2000/svg" width="64" viewBox="0 0 32 16"/>"#, (64.0, 32.0)),
            (r#"<svg xmlns="http://www.w3.org/2000/svg" width="100%" height="100%" viewBox="0 0 10 20"/>"#, (10.0, 20.0)),
            (r#"<?xml version="1.0"?><!-- c --><svg xmlns="http://www.w3.org/2000/svg" width="3" height="4"/>"#, (3.0, 4.0)),
        ];
        for (src, want) in cases {
            let img = web::decode(src.as_bytes().to_vec()).await.unwrap_or_else(|e| panic!("{src}: {e}"));
            assert_eq!(img.natural_size(), want, "{src}");
        }
    }

    /// Undecodable bytes are an `Err` (→ `on_error`), never a blank image.
    #[wasm_bindgen_test]
    async fn browser_rejects_undecodable_bytes() {
        for bad in [
            b"AAAA".to_vec(),
            b"\x89PNG\r\n\x1a\ntruncated".to_vec(),
            b"<svg xmlns='http://www.w3.org/2000/svg'><unclosed></svg>".to_vec(),
            b"<svg xmlns='http://www.w3.org/2000/svg'/>".to_vec(), // no size at all
        ] {
            assert!(web::decode(bad.clone()).await.is_err(), "{:?}", String::from_utf8_lossy(&bad));
        }
    }

    #[derive(Debug, Clone, PartialEq)]
    enum Event {
        Load(f32, f32),
        Error,
    }

    fn make_backend() -> WgpuBackend {
        let text = Rc::new(RefCell::new(crate::text::TextStore::new()));
        let fs = Rc::new(RefCell::new(glyphon::FontSystem::new()));
        WgpuBackend::new(
            text,
            fs,
            runtime_shared::ColorScheme::Light,
            Rc::new(crate::NativeSkin::new(runtime_shared::Platform::Web)),
        )
    }

    fn image(b: &mut WgpuBackend, src: &str) -> (WgpuNode, Rc<RefCell<Vec<Event>>>) {
        let node = b.create_image_impl(src, None, &Default::default());
        let log = Rc::new(RefCell::new(Vec::new()));
        let l = log.clone();
        b.install_image_load_handler_impl(
            &node,
            Rc::new(move |ev: &ImageLoadEvent| l.borrow_mut().push(Event::Load(ev.width, ev.height))),
        );
        let l = log.clone();
        b.install_image_error_handler_impl(&node, Rc::new(move || l.borrow_mut().push(Event::Error)));
        (node, log)
    }

    /// Pump the backend's completion queue (what `Host::tick` does) while
    /// yielding to the event loop, until no load is in flight.
    async fn settle(b: &mut WgpuBackend) {
        for _ in 0..400 {
            b.pump_image_loads();
            if !b.has_pending_image_loads() {
                return;
            }
            sleep_ms(5).await;
        }
        panic!("image load never settled");
    }

    /// Every synchronous conformance vector through the real backend path
    /// on the web: `data:` srcs decode in the browser and settle on a later
    /// pump with the same events and natural sizes as native.
    #[wasm_bindgen_test]
    async fn sync_vectors_follow_the_contract_in_the_browser() {
        install_executor();
        let mut b = make_backend();
        let mut nodes = Vec::new();
        for (src, expect) in vectors::SYNC {
            let (node, log) = image(&mut b, src);
            nodes.push((src, expect, node, log));
        }
        settle(&mut b).await;
        for (src, expect, _node, log) in &nodes {
            let want = match expect {
                Expect::Loads { width, height } => Event::Load(*width as f32, *height as f32),
                Expect::Errors => Event::Error,
            };
            assert_eq!(*log.borrow(), vec![want], "{src:?}");
        }
    }

    /// The browser decode is asynchronous, so a `data:` src is pending at
    /// mount and a src replaced before its decode lands never fires.
    #[wasm_bindgen_test]
    async fn stale_browser_decode_never_replaces_newer_src() {
        install_executor();
        let mut b = make_backend();
        let (node, log) = image(&mut b, vectors::PNG_1X1_BASE64);
        assert!(b.has_pending_image_loads(), "browser decode is asynchronous");
        b.update_image_src_impl(&node, vectors::SVG_2X1_PERCENT);
        settle(&mut b).await;
        assert_eq!(*log.borrow(), vec![Event::Load(2.0, 1.0)]);
        // The PNG still decoded and is cached: setting it settles at once.
        b.update_image_src_impl(&node, vectors::PNG_1X1_BASE64);
        assert!(!b.has_pending_image_loads());
        assert_eq!(*log.borrow(), vec![Event::Load(2.0, 1.0), Event::Load(1.0, 1.0)]);
    }
}
