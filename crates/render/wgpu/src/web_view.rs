//! In-process WebView backed by Blitz (Dioxus Labs' HTML/CSS engine).
//!
//! Pipeline mirrors [`crate::video`]:
//!
//! 1. Per-WebView worker thread spins up its own tokio runtime.
//! 2. The worker fetches the URL's HTML, builds an [`HtmlDocument`],
//!    resolves layout/style, waits for sub-resources (CSS, images,
//!    fonts) to load via [`blitz_net::Provider`].
//! 3. The worker drives a [`VelloImageRenderer`] to rasterize the
//!    page into an RGBA8 buffer.
//! 4. The buffer lands in a `Mutex<Option<PaintedPage>>` slot.
//! 5. The renderer's pre-pass uploads the slot into a per-node
//!    wgpu texture and composites via the standard image pipeline.
//!
//! Why GPU vello with CPU readback rather than the CPU rasterizer:
//! [`VelloImageRenderer`] still uses the GPU for the actual
//! rasterization — only the final blit lands in a CPU buffer that
//! we re-upload. For complex pages that's still meaningfully faster
//! than `anyrender_vello_cpu`, and (importantly) the bundled wgpu
//! version matches ours so we can share types if a follow-up
//! ever wants to skip the readback.
//!
//! Out of scope for Phase 1 (mirroring the video Phase 1 scope):
//! interactivity (clicks, forms, scroll), JS execution (Blitz
//! doesn't have it anyway), resize-after-mount.

#![cfg(blitz_active)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyrender::{ImageRenderer, PaintScene as _};
use anyrender_vello::VelloImageRenderer;
use blitz_dom::DocumentConfig;
use blitz_html::HtmlDocument;
use blitz_net::Provider;
use blitz_paint::paint_scene;
use blitz_traits::shell::{ColorScheme, Viewport};
use peniko::Color;
use peniko::Fill;
use peniko::kurbo::Rect;

/// One fully-rendered page snapshot — interleaved RGBA8.
pub struct PaintedPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Shared state between the worker thread and the renderer's
/// pre-pass. Same shape as `crate::video::VideoSharedState`.
pub struct WebViewSharedState {
    pub latest_paint: Mutex<Option<PaintedPage>>,
    pub shutdown: AtomicBool,
    pub paint_counter: AtomicU64,
    /// `Some(url)` requests the worker to drop the current
    /// document and navigate. Worker takes + clears.
    pub navigate_request: Mutex<Option<String>>,
}

impl WebViewSharedState {
    fn new() -> Self {
        Self {
            latest_paint: Mutex::new(None),
            shutdown: AtomicBool::new(false),
            paint_counter: AtomicU64::new(0),
            navigate_request: Mutex::new(None),
        }
    }
}

/// Owning handle to the worker thread + the latest paint.
pub struct WebView {
    pub shared: Arc<WebViewSharedState>,
    join: Mutex<Option<thread::JoinHandle<()>>>,
}

impl Drop for WebView {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl WebView {
    /// Spawn a worker that fetches `url` and renders it at
    /// `(width, height)` device pixels. Returns immediately.
    pub fn spawn(url: String, width: u32, height: u32) -> Self {
        let shared = Arc::new(WebViewSharedState::new());
        let shared_for_thread = shared.clone();
        let join = thread::Builder::new()
            .name(format!("webview:{}", short_label(&url)))
            .spawn(move || run_worker(url, width, height, shared_for_thread))
            .expect("spawn webview worker");
        Self {
            shared,
            join: Mutex::new(Some(join)),
        }
    }

    /// Ask the worker to drop its current document and load `url`.
    /// Quietly no-ops if the worker has shut down.
    pub fn navigate(&self, url: String) {
        if let Ok(mut g) = self.shared.navigate_request.lock() {
            *g = Some(url);
        }
    }

    /// Synchronous teardown — sets the shutdown flag, joins the
    /// worker. Safe to call multiple times. Same role as
    /// `VideoDecoder::shutdown`.
    pub fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::Release);
        let handle = self.join.lock().ok().and_then(|mut g| g.take());
        if let Some(j) = handle {
            let _ = j.join();
        }
    }
}

fn short_label(src: &str) -> &str {
    // Keep thread name within macOS's 64-char limit; lop the
    // scheme + grab whatever follows the last `/`.
    let trimmed = src.rsplit('/').next().unwrap_or(src);
    if trimmed.len() > 48 { &trimmed[..48] } else { trimmed }
}

fn run_worker(initial_url: String, width: u32, height: u32, shared: Arc<WebViewSharedState>) {
    // The blitz_net::Provider's fetch path is async; we host a
    // dedicated current-thread tokio runtime so the worker
    // doesn't depend on a global one being present.
    let rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[webview] tokio runtime: {e}");
            return;
        }
    };
    rt.block_on(async move {
        let mut current_url = initial_url;
        loop {
            if shared.shutdown.load(Ordering::Acquire) {
                return;
            }
            if let Err(e) = load_and_render(&current_url, width, height, &shared).await {
                eprintln!("[webview] {current_url}: {e}");
            }
            // Wait for either a shutdown or a navigation.
            // No periodic re-paint — content is static until the
            // app calls `navigate()` (Phase 1 scope). Follow-up
            // would tie this to "dirty" signals from the document.
            loop {
                if shared.shutdown.load(Ordering::Acquire) {
                    return;
                }
                let next = shared
                    .navigate_request
                    .lock()
                    .ok()
                    .and_then(|mut g| g.take());
                if let Some(u) = next {
                    current_url = u;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        }
    });
}

async fn load_and_render(
    url_str: &str,
    width: u32,
    height: u32,
    shared: &WebViewSharedState,
) -> Result<(), String> {
    // 1) Parse / normalize the URL. Bare hostnames (`example.com`)
    //    are upgraded to `https://example.com` for ergonomics —
    //    matches the screenshot example.
    let url = url::Url::parse(url_str)
        .or_else(|_| url::Url::parse(&format!("https://{url_str}")))
        .map_err(|e| format!("parse url: {e}"))?;
    let url_string = url.to_string();

    // 2) Fetch the HTML body. `file://` reads from disk; anything
    //    else goes over reqwest. blitz-net's Provider is plugged
    //    in below for *sub*-resource fetches (CSS / images /
    //    fonts), but it doesn't own the top document.
    let html = if url.scheme() == "file" {
        let path = url.path();
        std::fs::read_to_string(path).map_err(|e| format!("read {path}: {e}"))?
    } else {
        let client = reqwest::Client::new();
        let response = client
            .get(url.clone())
            .header(
                "User-Agent",
                "Mozilla/5.0 (X11; Linux x86_64; rv:60.0) Gecko/20100101 Firefox/81.0",
            )
            .send()
            .await
            .map_err(|e| format!("GET: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("HTTP {}", response.status()));
        }
        response.text().await.map_err(|e| format!("read body: {e}"))?
    };

    if shared.shutdown.load(Ordering::Acquire) {
        return Ok(());
    }

    // 3) Build the Blitz document. `scale = 1.0` for now; in a
    //    follow-up we'd thread the device's `physical_to_logical`
    //    factor through so HiDPI renders crisp.
    let scale: f32 = 1.0;
    let provider: Arc<Provider> = Arc::new(Provider::new(None));
    let viewport = Viewport::new(width, height, scale, ColorScheme::Light);
    let mut document = HtmlDocument::from_html(
        &html,
        DocumentConfig {
            base_url: Some(url_string),
            net_provider: Some(provider.clone() as _),
            viewport: Some(viewport),
            ..Default::default()
        },
    );

    // 4) Resolve layout/styles and pump the network provider
    //    until every sub-resource it queued is done. Spin loop
    //    yields between polls so the runtime can drive its
    //    own async fetches.
    let pump_step = Duration::from_millis(10);
    let mut attempts = 0u32;
    loop {
        if shared.shutdown.load(Ordering::Acquire) {
            return Ok(());
        }
        document.resolve(0.0);
        if provider.is_empty() {
            break;
        }
        tokio::time::sleep(pump_step).await;
        attempts += 1;
        // Hard ceiling so a hung sub-resource doesn't park the
        // worker forever. ~5 s at 10 ms per step.
        if attempts > 500 {
            eprintln!("[webview] gave up waiting for sub-resources after ~5s");
            break;
        }
    }
    document.as_mut().resolve(0.0);

    // 5) Rasterize. `VelloImageRenderer` runs vello on the GPU and
    //    hands back an RGBA buffer; we then upload to *our* wgpu
    //    texture for compositing. The double-trip (GPU → CPU →
    //    GPU) is wasteful but the only zero-config path for a
    //    second wgpu device that doesn't share state with ours.
    //    Follow-up: instantiate the renderer with our existing
    //    device and skip the readback.
    let mut renderer = VelloImageRenderer::new(width, height);
    let mut buffer: Vec<u8> = Vec::new();
    renderer.render_to_vec(
        |scene| {
            // White background — fills the texture before the
            // page paints over it, so transparent pages render
            // against white instead of whatever was last in the
            // texture memory.
            scene.fill(
                Fill::NonZero,
                Default::default(),
                Color::WHITE,
                Default::default(),
                &Rect::new(0.0, 0.0, width as f64, height as f64),
            );
            paint_scene(scene, document.as_mut(), scale as f64, width, height, 0, 0);
        },
        &mut buffer,
    );

    if shared.shutdown.load(Ordering::Acquire) {
        return Ok(());
    }

    // 6) Publish the painted page and bump the counter so the
    //    renderer's pre-pass picks it up. The redraw ping is the
    //    same cross-thread hop used by the video decoder.
    if let Ok(mut slot) = shared.latest_paint.lock() {
        *slot = Some(PaintedPage {
            width,
            height,
            rgba: buffer,
        });
    }
    shared.paint_counter.fetch_add(1, Ordering::Release);
    crate::scheduler::request_redraw();

    Ok(())
}
