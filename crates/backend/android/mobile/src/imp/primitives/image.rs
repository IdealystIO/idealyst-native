//! `Element::Image` — an `android.widget.ImageView` that actually loads its
//! `src`, matching the iOS / macOS image views (CLAUDE.md §7).
//!
//! # Sources
//!
//! Routing is `crate::image_policy::plan_load` (host-tested), on the shared
//! `backend_image_source::classify_src` rules every native backend uses:
//!
//! - **`asset://{id}`** (`image_asset(LOGO)`): `register_asset` decoded the
//!   embedded bytes once into the backend's [`ImageCache`] (a `Bitmap`, or a
//!   parsed SVG); `create_image` / `update_image_src` look the id up.
//! - **`data:` URI** (base64 or percent-encoded): decoded synchronously via
//!   `backend_image_source::parse_data_uri`.
//! - **`http(s)://`**: fetched on a small background worker pool with
//!   `HttpURLConnection` over JNI (the main thread may not touch the network
//!   — `NetworkOnMainThreadException`), decoded there with
//!   `BitmapFactory.decodeByteArray`, and handed back to the main looper via
//!   the cooperative async executor (`runtime_shared::driver::spawn_async` —
//!   the worker wakes the future; its `TaskWaker` posts a `RustAsyncPoll` to
//!   the main `Handler`). No Glide/Coil dependency. Needs the app's
//!   `android.permission.INTERNET` (declared via the app manifest's
//!   permissions); cleartext `http://` additionally needs a network-security
//!   exception, like iOS ATS.
//!
//! Bytes are sniffed for SVG first (`backend_image_source::decode_bytes`);
//! everything else goes to `BitmapFactory`, which reads PNG, JPEG, WebP, GIF
//! (first frame), BMP and (API 28+) HEIF. An SVG is kept on the view and
//! rasterized with resvg at the view's displayed pixel size; the layout pass
//! re-rasterizes it when that size changes (`AndroidBackend::run_layout_pass`
//! → [`sync_svg_raster`]), so a vector logo stays sharp at any size.
//!
//! # Size and events
//!
//! The view's Taffy measure function reports the image's natural size in dp
//! (a bitmap's pixel count read as dp — the `UIImage` scale-1 / web
//! `naturalWidth` convention — or the SVG's intrinsic size), with an explicit
//! style dimension winning per axis, exactly like iOS. `on_load` fires with
//! that natural size; a source that can't be loaded or decoded fires
//! `on_error` and logs a warning instead of leaving a silently blank slot.
//! Handlers installed after the outcome (the embedded-asset order) fire
//! immediately, like iOS.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use backend_android_core::helpers::apply_default_layout_params;
use backend_image_source::{self as image_source, DecodedSource, SvgImage};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;
use runtime_shared::{
    AssetId, AssetSource, AssetTag, ImageErrorHandler, ImageLoadEvent, ImageLoadHandler, LogLevel,
    ObjectFit,
};

use crate::image_policy::{self, LoadPlan};
use crate::imp::{with_env, AndroidBackend};

/// Decoded registered assets, keyed by [`AssetId`]. Filled by
/// `register_asset` (`AssetTag::Image`), read when a `src` is
/// `asset://{id}`. A bitmap is decoded once; an SVG is kept parsed because
/// each view rasterizes it at its own size.
pub(crate) type ImageCache = HashMap<AssetId, CachedImage>;

#[derive(Clone)]
pub(crate) enum CachedImage {
    Bitmap { bitmap: GlobalRef, natural: (f32, f32) },
    Svg(Rc<SvgImage>),
}

/// Live image views, keyed by node key (the `GlobalRef`'s raw pointer —
/// `AndroidBackend::node_key_of`). The layout pass reads it to re-raster
/// SVGs; the walker's handler installs read it to find a view's state.
pub(crate) type ImageStates = HashMap<usize, Rc<ImageState>>;

/// Per-view load state. Shared (`Rc`) between the backend's [`ImageStates`]
/// table, the view's Taffy measure function, and any in-flight remote load,
/// so none of them needs the backend borrowed.
pub(crate) struct ImageState {
    view: GlobalRef,
    on_load: RefCell<Option<ImageLoadHandler>>,
    on_error: RefCell<Option<ImageErrorHandler>>,
    /// Latched on failure so an `on_error` installed afterwards still fires.
    errored: Cell<bool>,
    /// The `src` displayed / in flight — `image_policy::plan_load`'s
    /// unchanged-`src` guard.
    current_src: RefCell<Option<String>>,
    /// Natural size in dp, once something decoded. Read by the measure fn.
    natural: Cell<Option<(f32, f32)>>,
    /// The SVG on display, kept so the layout pass can re-rasterize it.
    svg: RefCell<Option<Rc<SvgImage>>>,
    /// Pixels per intrinsic dp of the current SVG raster (0 = none).
    svg_raster_scale: Cell<f64>,
    /// Bumped on every `src` change. A remote load captures it and drops
    /// its result if the view has moved on to another `src` meanwhile —
    /// otherwise a slow first fetch could overwrite a newer image.
    generation: Cell<u64>,
}

impl ImageState {
    fn key(&self) -> usize {
        AndroidBackend::node_key_of(&self.view)
    }
}

thread_local! {
    /// Image views whose natural size changed since the last layout pass.
    /// `run_layout_pass` drains it and `mark_dirty`s their Taffy nodes —
    /// Taffy caches leaf measurements, so without the dirty mark an image
    /// that finished loading would keep its pre-load 0×0 box. A queue
    /// (rather than marking at the call site) because an async load
    /// completes on a looper task with no `&mut AndroidBackend` in hand.
    static NATURAL_SIZE_DIRTY: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// Drain [`NATURAL_SIZE_DIRTY`] (called at the top of each layout pass).
pub(crate) fn take_natural_size_dirty() -> Vec<usize> {
    NATURAL_SIZE_DIRTY.with(|d| std::mem::take(&mut *d.borrow_mut()))
}

fn natural_size_changed(state: &ImageState) {
    NATURAL_SIZE_DIRTY.with(|d| d.borrow_mut().push(state.key()));
    crate::imp::scheduler::schedule_layout_pass_retry(0);
}

// ---------------------------------------------------------------------------
// Create / update
// ---------------------------------------------------------------------------

pub(crate) fn create(b: &mut AndroidBackend, src: &str, _alt: Option<&str>) -> GlobalRef {
    let view = with_env(|env| {
        let class = env.find_class("android/widget/ImageView").unwrap();
        let local = env
            .new_object(
                &class,
                "(Landroid/content/Context;)V",
                &[JValue::Object(&b.context.as_obj())],
            )
            .unwrap();
        apply_default_layout_params(env, &local);
        // Default the fit to Contain (aspect-fit) so a bare Android image
        // matches the framework-wide default and the other backends. An
        // `ImageView` defaults to `FIT_CENTER` (also aspect-fit), but we set
        // it explicitly so the value is known and a later `apply_style`
        // reset to `None` restores a consistent baseline.
        apply_object_fit(env, &local, ObjectFit::Contain);
        env.new_global_ref(local).unwrap()
    });
    let state = Rc::new(ImageState {
        view: view.clone(),
        on_load: RefCell::new(None),
        on_error: RefCell::new(None),
        errored: Cell::new(false),
        current_src: RefCell::new(None),
        natural: Cell::new(None),
        svg: RefCell::new(None),
        svg_raster_scale: Cell::new(0.0),
        generation: Cell::new(0),
    });
    b.image_states.insert(state.key(), state.clone());

    // Natural-size measure: an explicit style dimension wins per axis,
    // else the decoded size (0×0 until something loads). Without it an
    // unsized image collapses to 0×0 in a flex column.
    let layout = b.layout_for_view(&view);
    let measured = state.clone();
    b.layout.set_measure_fn(
        layout,
        Rc::new(move |known, _available| {
            let (w, h) = image_policy::measure(
                (known.width, known.height),
                measured.natural.get(),
            );
            runtime_layout::Size { width: w, height: h }
        }),
    );

    load(&b.image_cache, &state, src);
    view
}

/// Reactive `src` swap. A no-op for the `src` already displayed / in flight.
pub(crate) fn update_src(b: &mut AndroidBackend, node: &GlobalRef, src: &str) {
    let Some(state) = b.image_states.get(&AndroidBackend::node_key_of(node)).cloned() else {
        return;
    };
    load(&b.image_cache, &state, src);
}

fn load(cache: &ImageCache, state: &Rc<ImageState>, src: &str) {
    let plan = image_policy::plan_load(state.current_src.borrow().as_deref(), src);
    if plan == LoadPlan::Unchanged {
        return;
    }
    *state.current_src.borrow_mut() = Some(src.to_string());
    state.generation.set(state.generation.get().wrapping_add(1));
    match plan {
        LoadPlan::Unchanged => {}
        LoadPlan::Asset(id) => match cache.get(&AssetId(id)) {
            Some(CachedImage::Bitmap { bitmap, natural }) => {
                with_env(|env| show_bitmap(env, state, bitmap.as_obj()));
                loaded(state, *natural);
            }
            Some(CachedImage::Svg(svg)) => show_svg(state, svg.clone()),
            None => report_error(
                state,
                &format!("image: asset {id} is not registered (or its bytes didn't decode)"),
            ),
        },
        LoadPlan::DataUri => load_data_uri(state, src),
        LoadPlan::Remote(url) => load_remote(state, url),
        LoadPlan::Unsupported => report_error(
            state,
            &format!("image: unsupported src {:?} (expected asset://, data:, or http(s)://)", short(src)),
        ),
    }
}

fn load_data_uri(state: &Rc<ImageState>, src: &str) {
    let Some(uri) = image_source::parse_data_uri(src) else {
        report_error(state, &format!("image: malformed data URI ({}…)", short(src)));
        return;
    };
    match image_source::decode_bytes(&uri.bytes) {
        Some(DecodedSource::Svg(svg)) => show_svg(state, Rc::new(svg)),
        Some(DecodedSource::Bitmap(bytes)) => {
            let shown = with_env(|env| {
                let (bitmap, natural) = decode_bitmap(env, &bytes)?;
                show_bitmap(env, state, &bitmap);
                Some(natural)
            });
            match shown {
                Some(natural) => loaded(state, natural),
                None => report_error(
                    state,
                    &format!("image: could not decode data URI ({}…)", short(src)),
                ),
            }
        }
        None => report_error(state, &format!("image: data URI is not a valid SVG ({}…)", short(src))),
    }
}

/// First 48 chars of a `src`, for log lines (a data URI can be megabytes).
fn short(src: &str) -> &str {
    let end = src.char_indices().nth(48).map(|(i, _)| i).unwrap_or(src.len());
    &src[..end]
}

// ---------------------------------------------------------------------------
// Outcomes
// ---------------------------------------------------------------------------

/// Put a decoded `Bitmap` on the view, replacing any SVG it was showing.
/// The caller then fires [`loaded`] — outside its `with_env`, so author
/// `on_load` code never runs nested inside a backend JNI scope.
fn show_bitmap(env: &mut JNIEnv, state: &ImageState, bitmap: &JObject) {
    state.svg.borrow_mut().take();
    state.svg_raster_scale.set(0.0);
    set_image_bitmap(env, &state.view, bitmap);
}

/// Display `svg`: rasterize at the view's current size × density, keep the
/// document for re-rasters, fire `on_load` with the intrinsic size.
fn show_svg(state: &ImageState, svg: Rc<SvgImage>) {
    let ok = with_env(|env| {
        let density = super::super::cached_density(env, &state.view.as_obj());
        let frame = view_size_dp(env, &state.view, density);
        let scale = svg.raster_scale((frame.0 as f64, frame.1 as f64), density as f64);
        match raster_bitmap(env, &svg, scale) {
            Some(bitmap) => {
                set_image_bitmap(env, &state.view, bitmap.as_obj());
                state.svg_raster_scale.set(scale);
                true
            }
            None => false,
        }
    });
    if !ok {
        report_error(state, "image: SVG has no drawable area (zero intrinsic size)");
        return;
    }
    let (w, h) = svg.intrinsic_size();
    *state.svg.borrow_mut() = Some(svg);
    loaded(state, (w as f32, h as f32));
}

fn loaded(state: &ImageState, natural: (f32, f32)) {
    state.errored.set(false);
    if state.natural.replace(Some(natural)) != Some(natural) {
        natural_size_changed(state);
    }
    let handler = state.on_load.borrow().clone();
    if let Some(h) = handler {
        h(&ImageLoadEvent { width: natural.0, height: natural.1 });
    }
}

fn report_error(state: &ImageState, msg: &str) {
    runtime_shared::log(LogLevel::Warn, &format!("[backend-android] {msg} — firing on_error"));
    state.errored.set(true);
    let handler = state.on_error.borrow().clone();
    if let Some(h) = handler {
        h();
    }
}

/// Install `on_load`, firing at once if the view already holds an image
/// (an embedded asset / data URI decodes inside `create_image`, before the
/// walker installs handlers).
pub(crate) fn install_load_handler(b: &AndroidBackend, node: &GlobalRef, handler: ImageLoadHandler) {
    let Some(state) = b.image_states.get(&AndroidBackend::node_key_of(node)).cloned() else {
        return;
    };
    *state.on_load.borrow_mut() = Some(handler.clone());
    if let Some((w, h)) = state.natural.get() {
        if !state.errored.get() {
            handler(&ImageLoadEvent { width: w, height: h });
        }
    }
}

/// Install `on_error`, firing at once if the load already failed.
pub(crate) fn install_error_handler(b: &AndroidBackend, node: &GlobalRef, handler: ImageErrorHandler) {
    let Some(state) = b.image_states.get(&AndroidBackend::node_key_of(node)).cloned() else {
        return;
    };
    *state.on_error.borrow_mut() = Some(handler.clone());
    if state.errored.get() {
        handler();
    }
}

/// Drop a view's load state (its node is being torn down). An in-flight
/// remote load keeps its own `Rc` and lands harmlessly on the detached view.
pub(crate) fn forget(b: &mut AndroidBackend, node: &GlobalRef) {
    b.image_states.remove(&AndroidBackend::node_key_of(node));
}

/// Layout-pass hook: re-rasterize a displayed SVG when the view's laid-out
/// size (dp) needs a different density than its current raster. Plain
/// `setImageBitmap` — the natural size is unchanged, so neither `on_load`
/// nor layout reruns. Mirrors iOS `layoutSubviews` / macOS `setFrameSize:`.
pub(crate) fn sync_svg_raster(env: &mut JNIEnv, state: &ImageState, frame_dp: (f32, f32), density: f32) {
    let svg = state.svg.borrow().clone();
    let Some(svg) = svg else { return };
    let Some(scale) =
        image_policy::svg_reraster_scale(&svg, frame_dp, density, state.svg_raster_scale.get())
    else {
        return;
    };
    if let Some(bitmap) = raster_bitmap(env, &svg, scale) {
        set_image_bitmap(env, &state.view, bitmap.as_obj());
        state.svg_raster_scale.set(scale);
    }
}

// ---------------------------------------------------------------------------
// Assets
// ---------------------------------------------------------------------------

/// Decode an `AssetTag::Image` asset once and cache it by id. Bundled /
/// Remote sources (no embedded bytes) aren't cached — same as iOS — and a
/// later `asset://{id}` for them reports `on_error`.
pub(crate) fn register_asset(cache: &mut ImageCache, id: AssetId, kind: AssetTag, source: &AssetSource) {
    if kind != AssetTag::Image || cache.contains_key(&id) {
        return;
    }
    let bytes: &[u8] = match source {
        AssetSource::Embedded { bytes, .. } | AssetSource::BundledEmbedded { bytes, .. } => bytes,
        _ => return,
    };
    let cached = match image_source::decode_bytes(bytes) {
        Some(DecodedSource::Svg(svg)) => CachedImage::Svg(Rc::new(svg)),
        Some(DecodedSource::Bitmap(bytes)) => {
            match with_env(|env| {
                decode_bitmap(env, &bytes).and_then(|(bitmap, natural)| {
                    env.new_global_ref(bitmap).ok().map(|g| (g, natural))
                })
            }) {
                Some((bitmap, natural)) => CachedImage::Bitmap { bitmap, natural },
                None => {
                    runtime_shared::log(
                        LogLevel::Warn,
                        &format!("[backend-android] image asset {} did not decode", id.0),
                    );
                    return;
                }
            }
        }
        None => {
            runtime_shared::log(
                LogLevel::Warn,
                &format!("[backend-android] image asset {} sniffs as SVG but does not parse", id.0),
            );
            return;
        }
    };
    cache.insert(id, cached);
}

// ---------------------------------------------------------------------------
// Remote fetch
// ---------------------------------------------------------------------------

/// What a background fetch hands back to the main looper. `GlobalRef` is
/// `Send`, so a bitmap decoded on the worker crosses as-is; SVG crosses as
/// bytes and is parsed + rasterized on main (it needs the view's size).
#[cfg(feature = "async-driver")]
enum Fetched {
    Bitmap { bitmap: GlobalRef, natural: (f32, f32) },
    Svg(Vec<u8>),
    Failed(String),
}

#[cfg(feature = "async-driver")]
fn load_remote(state: &Rc<ImageState>, url: &str) {
    use std::sync::{Arc, Mutex};
    let slot = Arc::new(Mutex::new(fetch::Slot::default()));
    fetch::submit(url.to_string(), slot.clone());
    let state = state.clone();
    let generation = state.generation.get();
    runtime_shared::driver::spawn_async(async move {
        let fetched = fetch::SlotFuture(slot).await;
        if state.generation.get() != generation {
            // The view moved on to another `src` while this was in flight.
            return;
        }
        match fetched {
            Fetched::Bitmap { bitmap, natural } => {
                with_env(|env| show_bitmap(env, &state, bitmap.as_obj()));
                loaded(&state, natural);
            }
            Fetched::Svg(bytes) => match SvgImage::parse(&bytes) {
                Some(svg) => show_svg(&state, Rc::new(svg)),
                None => report_error(&state, "image: fetched SVG does not parse"),
            },
            Fetched::Failed(why) => report_error(&state, &format!("image: {why}")),
        }
    });
}

/// Without the cooperative executor there is no way back onto the main
/// looper from a worker. The CLI's Android builds always enable
/// `async-driver`; this only fires for a hand-rolled build that doesn't.
#[cfg(not(feature = "async-driver"))]
fn load_remote(state: &Rc<ImageState>, url: &str) {
    report_error(
        state,
        &format!("image: remote src {url:?} needs the backend's `async-driver` feature"),
    );
}

#[cfg(feature = "async-driver")]
mod fetch {
    //! A small fixed worker pool for remote image fetches. Fixed rather than
    //! thread-per-image so a grid of 100 thumbnails doesn't spawn 100
    //! threads (and 100 concurrent connections).

    use super::{decode_bitmap, Fetched};
    use backend_image_source as image_source;
    use jni::objects::{JObject, JValue};
    use jni::JNIEnv;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::task::{Context, Poll, Waker};

    /// Concurrent fetches. Browsers allow ~6 connections per host; 4 keeps
    /// a phone's radio busy without starving the app's own requests.
    const FETCH_WORKERS: usize = 4;
    /// Connect / read timeout per fetch, ms.
    const FETCH_TIMEOUT_MS: i32 = 15_000;
    /// Refuse bodies past this size: decoding a huge image would allocate a
    /// bitmap of `w × h × 4` bytes and can OOM the app.
    const MAX_IMAGE_BYTES: usize = 32 * 1024 * 1024;
    const READ_CHUNK: i32 = 64 * 1024;

    #[derive(Default)]
    pub(super) struct Slot {
        result: Option<Fetched>,
        waker: Option<Waker>,
    }

    /// Resolves once the worker has filled the slot. Polled on the main
    /// looper by the cooperative executor; the worker's `wake()` is what
    /// posts the re-poll there.
    pub(super) struct SlotFuture(pub(super) Arc<Mutex<Slot>>);

    impl Future for SlotFuture {
        type Output = Fetched;
        fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Fetched> {
            let mut slot = self.0.lock().unwrap_or_else(|p| p.into_inner());
            match slot.result.take() {
                Some(r) => Poll::Ready(r),
                None => {
                    slot.waker = Some(cx.waker().clone());
                    Poll::Pending
                }
            }
        }
    }

    type Job = (String, Arc<Mutex<Slot>>);

    static QUEUE: OnceLock<Mutex<Sender<Job>>> = OnceLock::new();

    pub(super) fn submit(url: String, slot: Arc<Mutex<Slot>>) {
        let queue = QUEUE.get_or_init(|| {
            let (tx, rx) = channel::<Job>();
            let rx = Arc::new(Mutex::new(rx));
            for i in 0..FETCH_WORKERS {
                let rx = rx.clone();
                let _ = std::thread::Builder::new()
                    .name(format!("idealyst-image-{i}"))
                    .spawn(move || worker(rx));
            }
            Mutex::new(tx)
        });
        let _ = queue.lock().unwrap_or_else(|p| p.into_inner()).send((url, slot));
    }

    fn worker(rx: Arc<Mutex<Receiver<Job>>>) {
        loop {
            // The guard is a temporary: the lock is released before the
            // (slow) fetch runs, so the other workers keep pulling jobs.
            let job = rx.lock().unwrap_or_else(|p| p.into_inner()).recv();
            let Ok((url, slot)) = job else { return };
            // A panic must still resolve the slot — otherwise the view's
            // future stays pending forever and `on_error` never fires.
            let fetched = std::panic::catch_unwind(|| fetch_and_decode(&url))
                .unwrap_or_else(|_| Fetched::Failed(format!("fetch of {url} panicked")));
            let mut s = slot.lock().unwrap_or_else(|p| p.into_inner());
            s.result = Some(fetched);
            if let Some(w) = s.waker.take() {
                w.wake();
            }
        }
    }

    fn fetch_and_decode(url: &str) -> Fetched {
        // `with_env` attaches this worker to the JVM permanently (detached
        // on thread exit). Only platform classes are touched here, which
        // the system class loader a native thread gets can resolve.
        crate::imp::with_env(|env| {
            let result = (|| -> Result<Fetched, String> {
                let bytes = http_get(env, url)?;
                if image_source::looks_like_svg(&bytes) {
                    return Ok(Fetched::Svg(bytes));
                }
                let (bitmap, natural) = decode_bitmap(env, &bytes)
                    .ok_or_else(|| format!("{url}: bytes are not a decodable image"))?;
                let bitmap = env.new_global_ref(bitmap).map_err(|e| e.to_string())?;
                Ok(Fetched::Bitmap { bitmap, natural })
            })();
            // Any throwing JNI call leaves its exception PENDING; a worker
            // that returns to its loop (or exits) with one pending crashes
            // the process — see `project_android_net_pending_exception_clear`.
            let pending = take_pending_exception(env);
            match result {
                Ok(f) => f,
                Err(e) => Fetched::Failed(match pending {
                    Some(ex) => format!("{url}: {ex}"),
                    None => e,
                }),
            }
        })
    }

    /// `HttpURLConnection` GET → body bytes. Errors carry a short reason;
    /// the caller clears any pending Java exception.
    fn http_get(env: &mut JNIEnv, url: &str) -> Result<Vec<u8>, String> {
        let e = |err: jni::errors::Error| err.to_string();
        let jurl = env.new_string(url).map_err(e)?;
        let url_obj = env
            .new_object("java/net/URL", "(Ljava/lang/String;)V", &[JValue::Object(&jurl)])
            .map_err(e)?;
        let conn = env
            .call_method(&url_obj, "openConnection", "()Ljava/net/URLConnection;", &[])
            .and_then(|v| v.l())
            .map_err(e)?;
        env.call_method(&conn, "setConnectTimeout", "(I)V", &[JValue::Int(FETCH_TIMEOUT_MS)])
            .map_err(e)?;
        env.call_method(&conn, "setReadTimeout", "(I)V", &[JValue::Int(FETCH_TIMEOUT_MS)])
            .map_err(e)?;
        let status = env
            .call_method(&conn, "getResponseCode", "()I", &[])
            .and_then(|v| v.i())
            .map_err(e)?;
        if !(200..300).contains(&status) {
            let _ = env.call_method(&conn, "disconnect", "()V", &[]);
            return Err(format!("{url}: HTTP {status}"));
        }
        let body = read_all(env, &conn);
        let _ = env.call_method(&conn, "disconnect", "()V", &[]);
        body
    }

    fn read_all(env: &mut JNIEnv, conn: &JObject) -> Result<Vec<u8>, String> {
        let e = |err: jni::errors::Error| err.to_string();
        let stream = env
            .call_method(conn, "getInputStream", "()Ljava/io/InputStream;", &[])
            .and_then(|v| v.l())
            .map_err(e)?;
        let buf = env.new_byte_array(READ_CHUNK).map_err(e)?;
        let mut out: Vec<u8> = Vec::new();
        let mut chunk = vec![0i8; READ_CHUNK as usize];
        loop {
            let n = env
                .call_method(&stream, "read", "([B)I", &[JValue::Object(&buf)])
                .and_then(|v| v.i())
                .map_err(e)?;
            if n < 0 {
                break;
            }
            let n = n as usize;
            env.get_byte_array_region(&buf, 0, &mut chunk[..n]).map_err(e)?;
            out.extend(chunk[..n].iter().map(|&b| b as u8));
            if out.len() > MAX_IMAGE_BYTES {
                let _ = env.call_method(&stream, "close", "()V", &[]);
                return Err(format!("body exceeds {MAX_IMAGE_BYTES} bytes"));
            }
        }
        let _ = env.call_method(&stream, "close", "()V", &[]);
        Ok(out)
    }

    /// Capture the pending throwable's `toString()` and CLEAR it. Only
    /// `exception_occurred` / `exception_clear` are legal while one is
    /// pending, so clear first, then describe.
    fn take_pending_exception(env: &mut JNIEnv) -> Option<String> {
        if !env.exception_check().unwrap_or(false) {
            return None;
        }
        let ex = env.exception_occurred().ok();
        let _ = env.exception_clear();
        let ex = ex?;
        let s = env
            .call_method(&ex, "toString", "()Ljava/lang/String;", &[])
            .and_then(|v| v.l())
            .ok()?;
        let _ = env.exception_clear();
        let s = jni::objects::JString::from(s);
        env.get_string(&s).ok().map(|s| s.into())
    }
}

// ---------------------------------------------------------------------------
// JNI helpers
// ---------------------------------------------------------------------------

/// `BitmapFactory.decodeByteArray` → `(bitmap, natural size)`, `None` when
/// the bytes aren't an image the platform decodes. No `Options`: the bitmap
/// keeps its true pixel count (no density scaling), which is the natural
/// size in dp by the `UIImage` scale-1 convention. Safe off the main thread.
fn decode_bitmap<'l>(env: &mut JNIEnv<'l>, bytes: &[u8]) -> Option<(JObject<'l>, (f32, f32))> {
    let arr = env.byte_array_from_slice(bytes).ok()?;
    let bitmap = env
        .call_static_method(
            "android/graphics/BitmapFactory",
            "decodeByteArray",
            "([BII)Landroid/graphics/Bitmap;",
            &[JValue::Object(&arr), JValue::Int(0), JValue::Int(bytes.len() as i32)],
        )
        .and_then(|v| v.l())
        .ok();
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
        return None;
    }
    let bitmap = bitmap.filter(|b| !b.is_null())?;
    let w = env.call_method(&bitmap, "getWidth", "()I", &[]).and_then(|v| v.i()).ok()?;
    let h = env.call_method(&bitmap, "getHeight", "()I", &[]).and_then(|v| v.i()).ok()?;
    Some((bitmap, (w as f32, h as f32)))
}

/// Rasterize `svg` at `scale` px per intrinsic dp into an `ARGB_8888`
/// `Bitmap`. resvg's pixmap is premultiplied RGBA in memory order, which is
/// exactly `ARGB_8888`'s in-memory layout (premultiplied, R,G,B,A bytes), so
/// `copyPixelsFromBuffer` takes it without conversion. The direct buffer
/// only aliases `raster`'s Vec for the duration of the synchronous copy.
fn raster_bitmap(env: &mut JNIEnv, svg: &SvgImage, scale: f64) -> Option<GlobalRef> {
    let mut raster = svg.rasterize(scale)?;
    let config = env
        .get_static_field(
            "android/graphics/Bitmap$Config",
            "ARGB_8888",
            "Landroid/graphics/Bitmap$Config;",
        )
        .and_then(|v| v.l())
        .ok()?;
    let bitmap = env
        .call_static_method(
            "android/graphics/Bitmap",
            "createBitmap",
            "(IILandroid/graphics/Bitmap$Config;)Landroid/graphics/Bitmap;",
            &[
                JValue::Int(raster.width_px as i32),
                JValue::Int(raster.height_px as i32),
                JValue::Object(&config),
            ],
        )
        .and_then(|v| v.l())
        .ok()?;
    // SAFETY: the buffer views `raster.rgba_premultiplied`, which outlives
    // the synchronous `copyPixelsFromBuffer` call; nothing retains it after.
    let buffer = unsafe {
        env.new_direct_byte_buffer(
            raster.rgba_premultiplied.as_mut_ptr(),
            raster.rgba_premultiplied.len(),
        )
    }
    .ok()?;
    env.call_method(
        &bitmap,
        "copyPixelsFromBuffer",
        "(Ljava/nio/Buffer;)V",
        &[JValue::Object(&buffer)],
    )
    .ok()?;
    if env.exception_check().unwrap_or(false) {
        let _ = env.exception_clear();
        return None;
    }
    env.new_global_ref(bitmap).ok()
}

fn set_image_bitmap(env: &mut JNIEnv, view: &GlobalRef, bitmap: &JObject) {
    let _ = env.call_method(
        view.as_obj(),
        "setImageBitmap",
        "(Landroid/graphics/Bitmap;)V",
        &[JValue::Object(bitmap)],
    );
}

/// The view's current size in dp (`0×0` before its first layout).
fn view_size_dp(env: &mut JNIEnv, view: &GlobalRef, density: f32) -> (f32, f32) {
    let d = if density > 0.0 { density } else { 1.0 };
    let w = env.call_method(view.as_obj(), "getWidth", "()I", &[]).and_then(|v| v.i()).unwrap_or(0);
    let h = env.call_method(view.as_obj(), "getHeight", "()I", &[]).and_then(|v| v.i()).unwrap_or(0);
    (w as f32 / d, h as f32 / d)
}

// ---------------------------------------------------------------------------
// object_fit
// ---------------------------------------------------------------------------

/// The `ImageView.ScaleType` static-field name for an [`ObjectFit`].
fn scale_type_field(fit: ObjectFit) -> &'static str {
    match fit {
        ObjectFit::Fill => "FIT_XY",        // stretch to fill, ignore aspect
        ObjectFit::Contain => "FIT_CENTER", // aspect-fit, letterbox
        ObjectFit::Cover => "CENTER_CROP",  // aspect-fill, center-crop
    }
}

/// Return `true` if `view` is an `android.widget.ImageView` — lets the
/// generic `apply_style` path apply `object_fit` only to images.
pub(crate) fn is_image_view(env: &mut JNIEnv, view: &JObject) -> bool {
    env.find_class("android/widget/ImageView")
        .ok()
        .and_then(|c| env.is_instance_of(view, &c).ok())
        .unwrap_or(false)
}

/// Apply an [`ObjectFit`] to an `ImageView` via `setScaleType`. No-op on
/// non-image views (the caller in `apply_style` guards via [`is_image_view`],
/// but this re-checks so it's safe to call broadly). `CENTER_CROP` (Cover)
/// crops the overflow to the view's bounds for free — the Android analog of
/// CSS `object-fit: cover`.
pub(crate) fn apply_object_fit(env: &mut JNIEnv, view: &JObject, fit: ObjectFit) {
    // `setScaleType` is an `ImageView` method — calling it on any other View
    // throws (leaving a pending Java exception). Guard first.
    if !is_image_view(env, view) {
        return;
    }
    let Ok(scale_type_class) = env.find_class("android/widget/ImageView$ScaleType") else {
        return;
    };
    let st = env
        .get_static_field(
            &scale_type_class,
            scale_type_field(fit),
            "Landroid/widget/ImageView$ScaleType;",
        )
        .and_then(|v| v.l());
    if let Ok(st) = st {
        let _ = env.call_method(
            view,
            "setScaleType",
            "(Landroid/widget/ImageView$ScaleType;)V",
            &[JValue::Object(&st)],
        );
    }
}
