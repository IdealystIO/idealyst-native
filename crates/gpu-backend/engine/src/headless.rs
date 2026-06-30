//! Headless offscreen screenshot rendering.
//!
//! Renders a mounted UI tree to an offscreen wgpu texture (no window,
//! no swapchain) and reads the pixels back as RGBA / PNG. This is the
//! engine-side half of the "screenshot the app even when mocked" dev
//! tool: a [`Screenshotter`] owns a headless wgpu device + a [`Host`] +
//! a [`Renderer`], and the same wire receiver that drives a real client
//! can drive this `Host`'s backend (share its `Rc<RefCell<WgpuBackend>>`
//! via [`Screenshotter::backend`]).
//!
//! ## Why this matches the on-device output
//!
//! It runs the **same** wgpu `Renderer` and shaders the windowed host
//! uses — only the render target differs (an offscreen texture instead
//! of a swapchain surface). On a GPU-less server, [`Screenshotter::new`]
//! falls back to a software adapter (Mesa lavapipe / llvmpipe via
//! Vulkan, or DX-WARP); since it executes the identical shaders, the
//! software output matches the GPU output pixel-for-pixel — just
//! slower. There is no second rasterizer to keep in sync.
//!
//! ## Skin is intentionally minimal
//!
//! Views, text (bundled font, so it works with no system fonts),
//! buttons, images, gradients, borders, shadows, and animations render
//! exactly as on-device — that's the layout + content + style fidelity
//! a screenshot cares about, and it's skin-independent.
//!
//! The [`Painter`] "skin" is a **purely cosmetic** layer: it exists so
//! the *windowed* simulator can approximate a platform's look (device
//! bezel, status bar, the iOS-vs-Android styling of native widgets) for
//! a developer previewing e.g. an iOS app on Linux. The headless
//! screenshot path does not need that, so [`HeadlessSkin`] deliberately
//! draws no chrome and no-ops the native-widget hooks (toggle, slider,
//! text-input chrome, activity-indicator, keyboard, navigator header).
//! This is by design, not an unfinished TODO — don't reach for a
//! platform skin here unless a specific consumer actually wants the
//! cosmetic approximation baked into its screenshots.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::rc::Rc;

use glyphon::Buffer;
use runtime_core::scheduling::{
    install_scheduler, is_scheduler_installed, ScheduleHandle, Scheduler,
};
use runtime_core::{ColorScheme, Element};

use crate::keyboard::{KeySpec, LaidKey, LayoutMetrics};
use crate::painter::{NavigatorHeaderChrome, NavigatorHeaderHit, Painter};
use crate::pipeline::Instance as RectInstance;
use crate::text::StagedText;

// ---------------------------------------------------------------------------
// Headless scheduler — buffer microtasks so deferred builds run at a safe
// point, not inline.
// ---------------------------------------------------------------------------

thread_local! {
    /// Microtasks queued during a headless mount, drained by
    /// `mount`'s call to `drain_buffered_microtasks` once the walk's
    /// backend borrow is released.
    static HEADLESS_MICROTASKS: RefCell<VecDeque<Box<dyn FnOnce() + 'static>>> =
        RefCell::new(VecDeque::new());
}

/// A **buffering** scheduler for headless rendering.
///
/// Why buffering and not synchronous: SDKs that build chrome lazily — the
/// drawer/stack navigators call `schedule_microtask` from inside
/// `create_navigator`, which the walker invokes while holding
/// `backend.borrow_mut()`. The synchronous no-scheduler fallback would run
/// that microtask *immediately*, re-entering the walker (it re-borrows the
/// same backend to build the slot's child tree) and panicking with
/// "RefCell already borrowed". Buffering instead defers each microtask;
/// `runtime_core::mount` drains them via `drain_buffered_microtasks` after
/// the mount walk completes and the borrow is free, so the slot/screen
/// builds run safely. This is what lets a `DrawerNavigator` (or any
/// navigator) app rasterize headlessly on the GPU backend.
///
/// `after_ms` / `after_animation_frame` / `raf_loop` are inert: a static
/// one-frame capture has no animation clock to drive, so deferring an
/// animation callback that never fires is correct (and avoids spinning a
/// raf loop that would never terminate). Apps that need post-mount async
/// state for their *first* paint should drive frames through the windowed
/// host instead.
struct HeadlessScheduler;

struct InertHandle;
impl ScheduleHandle for InertHandle {
    fn cancel(&mut self) {}
}

impl Scheduler for HeadlessScheduler {
    fn schedule_microtask(&self, f: Box<dyn FnOnce() + 'static>) {
        HEADLESS_MICROTASKS.with(|q| q.borrow_mut().push_back(f));
    }

    fn after_animation_frame(&self, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
    }

    fn after_ms(&self, _delay_ms: i32, _f: Box<dyn FnOnce() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
    }

    fn raf_loop(&self, _f: Box<dyn FnMut() + 'static>) -> Box<dyn ScheduleHandle> {
        Box::new(InertHandle)
    }

    fn drain_buffered_microtasks(&self) {
        // Loop, not a single pass: a drained microtask (a slot builder)
        // commonly schedules further microtasks (nested components,
        // reactive scopes). Run until the queue is empty.
        loop {
            let next = HEADLESS_MICROTASKS.with(|q| q.borrow_mut().pop_front());
            match next {
                Some(f) => f(),
                None => break,
            }
        }
    }
}

/// Install the headless buffering scheduler unless a host already
/// installed one (e.g. the dev-server sidecar). Idempotent and safe to
/// call from every `Screenshotter` constructor.
fn ensure_headless_scheduler() {
    if !is_scheduler_installed() {
        install_scheduler(Box::new(HeadlessScheduler));
    }
}
use crate::{Host, Renderer, WgpuBackend};

/// Offscreen render target format. Non-sRGB on purpose: the windowed
/// host (`host-winit`) deliberately picks a non-sRGB swapchain so alpha
/// blending happens in raw sRGB numbers (matching CSS / UIView / iOS
/// CAGradientLayer behavior). Matching that here keeps headless
/// screenshots identical to what a user sees on the desktop preview.
/// The stored bytes are already sRGB-encoded, so they drop straight
/// into a (sRGB) PNG with no conversion.
const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// A no-chrome [`Painter`] for headless rendering: zero safe-area
/// insets, no device bezel, and no-op native-widget paints. The skin is
/// a cosmetic platform-approximation layer (see the module docs);
/// headless capture is about layout/content/style fidelity, which is
/// skin-independent, so drawing none is intentional. Views, text,
/// buttons, images, gradients, borders, shadows and animations render
/// via the `Renderer` directly (not the painter).
pub struct HeadlessSkin;

impl Painter for HeadlessSkin {
    // Native widgets — stubbed. The layout box still exists (the
    // framework laid it out and the renderer drew its background/border
    // if styled); only the skin-drawn knob/track/caret is absent.
    fn paint_toggle(
        &self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _t: f32,
        _tint: Option<[f32; 4]>,
        _rects: &mut Vec<RectInstance>,
    ) {
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_slider(
        &self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _value: f32,
        _min: f32,
        _max: f32,
        _tint: Option<[f32; 4]>,
        _rects: &mut Vec<RectInstance>,
    ) {
    }

    #[allow(clippy::too_many_arguments)]
    fn paint_text_input<'a>(
        &self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _is_focused: bool,
        _draw_caret: bool,
        _is_placeholder: bool,
        _buffer: &'a Buffer,
        _caret_x_local: f32,
        _text_color: [f32; 4],
        _field_bg: Option<[f32; 4]>,
        _rects: &mut Vec<RectInstance>,
        _texts: &mut Vec<StagedText<'a>>,
    ) {
    }

    fn paint_activity_indicator(
        &self,
        _x: f32,
        _y: f32,
        _w: f32,
        _h: f32,
        _phase: f32,
        _tint: Option<[f32; 4]>,
        _rects: &mut Vec<RectInstance>,
    ) {
    }

    // On-screen keyboard — never shown headlessly (no rows → the
    // layout engine produces nothing to paint).
    fn keyboard_rows(&self) -> Vec<Vec<KeySpec>> {
        Vec::new()
    }

    fn keyboard_layout_metrics(&self) -> LayoutMetrics {
        LayoutMetrics {
            key_gap: 0.0,
            row_gap: 0.0,
            side_margin: 0.0,
            vert_margin: 0.0,
        }
    }

    fn paint_keyboard<'a>(
        &self,
        _keyboard_rect: (f32, f32, f32, f32),
        _laid_keys: &[LaidKey],
        _pressed_label: Option<&'static str>,
        _glyphs: &'a HashMap<&'static str, Buffer>,
        _rects: &mut Vec<RectInstance>,
        _texts: &mut Vec<StagedText<'a>>,
    ) {
    }

    // Navigator header — stubbed. The screen body still renders; only
    // the skin-drawn title bar is absent.
    fn paint_navigator_header<'a, 'b>(
        &self,
        _rect: (f32, f32, f32, f32),
        _chrome: NavigatorHeaderChrome<'a, 'b>,
        _rects: &mut Vec<RectInstance>,
        _texts: &mut Vec<StagedText<'a>>,
        _hit_regions: &mut Vec<NavigatorHeaderHit>,
    ) {
    }
}

/// Owns a headless wgpu device + a [`Host`] + a [`Renderer`], and
/// produces RGBA / PNG screenshots of whatever is mounted on the host.
pub struct Screenshotter {
    // `instance` / `adapter` held so the device stays valid.
    _instance: wgpu::Instance,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    host: Host,
    renderer: Renderer,
    width: u32,
    height: u32,
    /// `true` if we're running on a software (fallback) adapter rather
    /// than real hardware. Output is identical; this is informational
    /// (e.g. to log slower capture on CI).
    pub software: bool,
}

impl Screenshotter {
    /// Build a headless renderer sized `width` × `height` (physical
    /// pixels). Prefers a hardware adapter; on a GPU-less host it
    /// retries with a software fallback adapter (lavapipe / WARP).
    /// Returns an error string if no adapter at all is available or the
    /// device request fails.
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        Self::with_color_scheme(width, height, ColorScheme::Light)
    }

    /// As [`Self::new`] but lets the caller pick the color scheme the
    /// backend reports to the app (affects `@media`-style theme reads).
    /// Uses the no-chrome [`HeadlessSkin`] (which reports `Custom("")`);
    /// for a capture that reports a real host-OS platform — so the app
    /// takes its native-desktop branch — use
    /// [`Self::with_color_scheme_and_skin`] with a
    /// [`crate::NativeSkin`].
    pub fn with_color_scheme(
        width: u32,
        height: u32,
        color_scheme: ColorScheme,
    ) -> Result<Self, String> {
        Self::with_color_scheme_and_skin(width, height, color_scheme, Rc::new(HeadlessSkin))
    }

    /// As [`Self::with_color_scheme`] but lets the caller supply the
    /// [`Painter`] skin. The skin's [`Painter::platform`] read-out is what
    /// author code sees via `runtime_core::platform()`, so passing a
    /// [`crate::NativeSkin`] built for a desktop OS makes the capture
    /// exercise the app's native-desktop layout (e.g. idea-ui-docs's
    /// pinned-sidebar branch under `Platform::MacOs`) rather than the
    /// mobile branch `HeadlessSkin`'s empty identity would select.
    pub fn with_color_scheme_and_skin(
        width: u32,
        height: u32,
        color_scheme: ColorScheme,
        skin: Rc<dyn Painter>,
    ) -> Result<Self, String> {
        // Navigator/lazy-chrome SDKs defer builds via `schedule_microtask`;
        // a buffering scheduler keeps those off the borrowed walk stack so
        // `mount` can drain them safely. See `HeadlessScheduler`.
        ensure_headless_scheduler();
        let width = width.max(1);
        let height = height.max(1);

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            flags: wgpu::InstanceFlags::default(),
            memory_budget_thresholds: Default::default(),
            backend_options: wgpu::BackendOptions::default(),
            display: None,
        });

        // No surface to be compatible with — this is the whole point of
        // headless. Try hardware first, then a software fallback.
        let (adapter, software) = match pollster::block_on(instance.request_adapter(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::default(),
                force_fallback_adapter: false,
                compatible_surface: None,
            },
        )) {
            Ok(a) => (a, false),
            Err(_) => {
                let a = pollster::block_on(instance.request_adapter(
                    &wgpu::RequestAdapterOptions {
                        power_preference: wgpu::PowerPreference::default(),
                        force_fallback_adapter: true,
                        compatible_surface: None,
                    },
                ))
                .map_err(|e| {
                    format!(
                        "no GPU and no software fallback adapter for headless render: {e}. \
                         On a Linux server install Mesa lavapipe (`libgl1-mesa-dri` / \
                         `mesa-vulkan-drivers`)."
                    )
                })?;
                (a, true)
            }
        };

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("idealyst-headless-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults().using_resolution(adapter.limits()),
            memory_hints: wgpu::MemoryHints::default(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            trace: wgpu::Trace::Off,
        }))
        .map_err(|e| format!("request_device failed for headless render: {e}"))?;

        let mut host = Host::new(skin, color_scheme);
        host.set_viewport(width as f32, height as f32);

        let renderer = Renderer::new(&device, &queue, TARGET_FORMAT);

        Ok(Self {
            _instance: instance,
            _adapter: adapter,
            device,
            queue,
            host,
            renderer,
            width,
            height,
            software,
        })
    }

    /// The backend the host renders. Share this `Rc` into a
    /// `dev_client::WireBackend::new_with_shared(...)` so a wire command
    /// stream replays into the very tree this screenshotter rasterizes.
    pub fn backend(&self) -> Rc<RefCell<WgpuBackend>> {
        self.host.backend().clone()
    }

    /// Mount an app directly (in-process, no wire). Keeps the reactive
    /// scope alive on the host until the screenshotter is dropped.
    pub fn mount<F>(&mut self, build_ui: F)
    where
        F: FnOnce() -> Element + 'static,
    {
        self.host.set_viewport(self.width as f32, self.height as f32);
        self.host.mount(build_ui);
    }

    /// Current target size in physical pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Render the current tree to an offscreen texture and read the
    /// pixels back as tightly-packed RGBA8 (`width * height * 4`
    /// bytes, row-major, no padding).
    pub fn capture_rgba(&mut self) -> Vec<u8> {
        let (w, h) = (self.width, self.height);

        let target = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("headless-screenshot-target"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = target.create_view(&wgpu::TextureViewDescriptor::default());

        // Same Renderer + shaders the windowed host runs; only the
        // target view differs. Logical == physical (scale 1.0), full
        // surface rect.
        self.host.set_viewport(w as f32, h as f32);
        self.renderer.render(
            &self.host,
            &self.device,
            &self.queue,
            &view,
            (w as f32, h as f32),
            (0.0, 0.0, w as f32, h as f32),
        );

        // copy_texture_to_buffer requires bytes_per_row to be a
        // multiple of 256, so the readback buffer is row-padded; we
        // strip the padding after mapping.
        let unpadded_bpr = w * 4;
        let padded_bpr = unpadded_bpr.div_ceil(256) * 256;

        let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("headless-screenshot-readback"),
            size: (padded_bpr as u64) * (h as u64),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("headless-screenshot-copy"),
            });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bpr),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
        );
        self.queue.submit([encoder.finish()]);

        // Map + block until the GPU finishes.
        let slice = readback.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());

        let data = slice.get_mapped_range();
        let mut out = Vec::with_capacity((unpadded_bpr as usize) * (h as usize));
        for row in 0..h {
            let start = (row * padded_bpr) as usize;
            let end = start + unpadded_bpr as usize;
            out.extend_from_slice(&data[start..end]);
        }
        drop(data);
        readback.unmap();
        out
    }

    /// Render the current tree and encode it as PNG bytes.
    pub fn capture_png(&mut self) -> Result<Vec<u8>, String> {
        let rgba = self.capture_rgba();
        let img = image::RgbaImage::from_raw(self.width, self.height, rgba)
            .ok_or_else(|| "captured buffer size does not match dimensions".to_string())?;
        let mut png = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .map_err(|e| format!("PNG encode failed: {e}"))?;
        Ok(png)
    }
}

/// One-shot convenience: build a headless renderer, mount `app`, and
/// return PNG bytes. For repeated captures (e.g. after reactive
/// updates) construct a [`Screenshotter`] and call `capture_png`
/// multiple times instead.
pub fn mount_and_capture_png<F>(width: u32, height: u32, app: F) -> Result<Vec<u8>, String>
where
    F: FnOnce() -> Element + 'static,
{
    let mut shot = Screenshotter::new(width, height)?;
    shot.mount(app);
    shot.capture_png()
}
