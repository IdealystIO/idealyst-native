//! Windows renderer for the canvas SDK — Direct2D.
//!
//! The Windows sibling of [`crate::macos`] (CoreGraphics) and
//! [`crate::linux`] (Cairo): it replays the canvas [`Scene`]'s [`DrawOp`]s
//! with the platform's own 2D engine. Direct2D runs on Direct3D 11 and falls
//! back to Windows' software rasterizer (WARP) by itself, so the canvas works
//! wherever the app does — a VM, Remote Desktop, a machine with no usable GPU
//! — which is the job `canvas-native` does on every platform.
//!
//! # Surface
//!
//! Views on Windows are painted by the backend, not native windows, so the
//! canvas takes a `graphics` node (`GraphicsOps::create_graphics`): the
//! backend gives it a DirectComposition visual in its composition tree,
//! positioned, clipped and stacked with the rest of the scene. The canvas
//! binds a premultiplied-alpha composition swapchain to that visual and
//! presents into it. Each frame is drawn into an offscreen bitmap first and
//! then copied to the swapchain: the blend modes that need the pixels
//! already drawn read them back from that bitmap (a swapchain buffer cannot
//! be read).
//!
//! # Coordinate space
//!
//! Scenes are in logical pixels, y down. The frame is device-sized
//! (`logical × scale`) and every replay starts from a `scale` base
//! transform, as the web and Android renderers do.
//!
//! # Clips and blend modes
//!
//! Direct2D's fast path only blends source-over, and a geometric clip is a
//! layer that composites its content source-over too. So an op is drawn
//! directly only when its blend is `Normal` and every clip in force is an
//! axis-aligned rectangle (`PushAxisAlignedClip` is a real clip, not a
//! layer). Anything else — a rounded or path clip, `DestinationOut`, the
//! separable and non-separable modes — is drawn into a scratch bitmap with
//! the clips applied there, then composited onto the target with the
//! op's blend: `DESTINATION_OUT` for the eraser, the Blend effect over a
//! copy of the target for the rest. Every `BlendMode` is exact; none
//! falls back to source-over.
//!
//! # Layers
//!
//! `DrawOp::Layer` / `LayerCached` are persistent device-sized bitmaps kept
//! between frames (keyed by id, in two separate maps so the two kinds can
//! share an id, like the Apple and Android renderers), rebuilt when the
//! canvas size changes. A `LayerCached` bakes on first sight or after a
//! rebuild even when the scene says it is clean, as Apple and Android do.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use backend_windows::{WindowsBackend, WindowsNode};
use canvas_core::{
    BlendMode, CanvasPrim, Color, DrawOp, FillRule, ImageSource, LineCap, LineJoin, Paint, PaintKind,
    Path, PathSeg, Scene, SizeReporter, Stroke, TextureLayer, Transform,
};
use runtime_scene::{Element, MountCx};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::graphics::{
    ComposedTarget, GraphicsTarget, OnLost, OnReady, OnReadyEvent, OnResize, OnResizeEvent,
};
use runtime_vocabulary::caps::GraphicsOps;

use windows::core::{Interface, Result};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_COMPOSITE_MODE_DESTINATION_OUT,
    D2D1_COMPOSITE_MODE_SOURCE_COPY, D2D1_COMPOSITE_MODE_SOURCE_OVER, D2D1_FIGURE_BEGIN_FILLED,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_FILL_MODE_ALTERNATE, D2D1_FILL_MODE_WINDING,
    D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F, D2D_SIZE_U, D2D1_BEZIER_SEGMENT,
    D2D1_BLEND_MODE, D2D1_BLEND_MODE_COLOR, D2D1_BLEND_MODE_COLOR_BURN, D2D1_BLEND_MODE_COLOR_DODGE,
    D2D1_BLEND_MODE_DARKEN, D2D1_BLEND_MODE_DIFFERENCE, D2D1_BLEND_MODE_EXCLUSION,
    D2D1_BLEND_MODE_HARD_LIGHT, D2D1_BLEND_MODE_HUE, D2D1_BLEND_MODE_LIGHTEN,
    D2D1_BLEND_MODE_LUMINOSITY, D2D1_BLEND_MODE_MULTIPLY, D2D1_BLEND_MODE_OVERLAY,
    D2D1_BLEND_MODE_SATURATION, D2D1_BLEND_MODE_SCREEN, D2D1_BLEND_MODE_SOFT_LIGHT,
};
use windows::Win32::Graphics::Direct2D::{
    CLSID_D2D1AlphaMask, CLSID_D2D1Blend, CLSID_D2D1LuminanceToAlpha, CLSID_D2D1Opacity,
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1Brush, ID2D1Device, ID2D1DeviceContext, ID2D1Factory,
    ID2D1Factory1, ID2D1Geometry, ID2D1Image, ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_BLEND_PROP_MODE, D2D1_CAP_STYLE, D2D1_CAP_STYLE_FLAT, D2D1_CAP_STYLE_ROUND,
    D2D1_CAP_STYLE_SQUARE, D2D1_DASH_STYLE_CUSTOM, D2D1_DASH_STYLE_SOLID,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_EXTEND_MODE_CLAMP, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_GAMMA_2_2, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_LAYER_OPTIONS1_NONE, D2D1_LAYER_PARAMETERS1,
    D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES, D2D1_LINE_JOIN_BEVEL, D2D1_LINE_JOIN_MITER_OR_BEVEL,
    D2D1_LINE_JOIN_ROUND, D2D1_OPACITY_PROP_OPACITY,
    D2D1_PROPERTY_TYPE_ENUM, D2D1_PROPERTY_TYPE_FLOAT, D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES,
    D2D1_QUADRATIC_BEZIER_SEGMENT, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::IDCompositionVisual;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

// Pure pixel-format conversions shared with the other renderers (tested on
// every host by `tests/pixels.rs`).
#[path = "pixels.rs"]
#[allow(dead_code)]
mod pixels;

// The fractional source-rect → destination mapping shared with Android
// (tested by `tests/layer_geometry.rs`).
#[path = "layer_geometry.rs"]
#[allow(dead_code)]
mod layer_geometry;

// ============================================================================
// Mount
// ============================================================================

pub(crate) fn mount_canvas(
    cx: &mut MountCx<'_, WindowsBackend>,
    prim: &Rc<CanvasPrim>,
    _children: Vec<Element>,
) -> WindowsNode {
    let backend = cx.backend().clone();
    let canvas = CanvasView::new(prim.size_reporter(), next_frame);

    let on_ready: OnReady = {
        let canvas = canvas.clone();
        Box::new(move |ev: OnReadyEvent| canvas.borrow_mut().ready(ev))
    };
    let on_resize: OnResize = {
        let canvas = canvas.clone();
        Box::new(move |ev: OnResizeEvent| canvas.borrow_mut().resize(ev))
    };
    let on_lost: OnLost = {
        let canvas = canvas.clone();
        Box::new(move || canvas.borrow_mut().lost())
    };
    let node = {
        let mut b = backend.borrow_mut();
        GraphicsOps::create_graphics(&mut *b, on_ready, on_resize, on_lost, &AccessibilityProps::default())
    };

    // Reactive repaint. Realize runs world-entered, so this effect is
    // collected into the mounting subtree and dies at unmount.
    let paint_prim = prim.clone();
    let props = prim.props.clone();
    runtime_world::effect(move || {
        let scene = paint_prim.paint();
        canvas.borrow_mut().install(scene, props.layers.clone());
        CanvasView::request_frame(&canvas);
    });

    crate::finish_mount(&backend, &node, prim);
    node
}

/// Schedules `f` for the next frame (`after_animation_frame`). A canvas
/// takes it as a parameter so its tests can run frames by hand.
type ScheduleFrame = fn(Box<dyn FnOnce()>) -> Option<runtime_shared::scheduling::ScheduledTask>;

fn next_frame(f: Box<dyn FnOnce()>) -> Option<runtime_shared::scheduling::ScheduledTask> {
    Some(runtime_shared::scheduling::after_animation_frame(f))
}

/// One mounted canvas: the latest scene and the window surface it is shown
/// on, once the backend has handed one over.
///
/// A new scene is drawn on the next frame, not when it arrives — the same
/// invalidate-then-paint every other renderer does (`setNeedsDisplay:`,
/// `queue_draw`). Drawing on arrival showed scenes the author never meant
/// to be seen: a chart's data change first recomputes its marks against
/// the settled transition clock (the END state), and only the follow-up
/// flush restarts the clock, so every animated chart flashed its end state
/// for a frame before animating. A frame also coalesces any number of
/// scene changes into one draw.
struct CanvasView {
    scene: Scene,
    textures: Vec<TextureLayer>,
    texture_subs: Vec<Option<canvas_core::Subscription>>,
    sizing: SizeReporter,
    /// The visual and size the backend gave us; kept across a device loss so
    /// the surface can be rebuilt without a new `on_ready`.
    binding: Option<Binding>,
    /// Device resources for `binding`; `None` until built (or after a loss).
    surface: Option<WindowSurface>,
    /// The installed scene has not been drawn yet.
    dirty: bool,
    /// A frame is scheduled to draw it.
    requested: bool,
    /// That frame's handle (dropping it cancels the frame).
    frame: Option<runtime_shared::scheduling::ScheduledTask>,
    schedule: ScheduleFrame,
    me: std::rc::Weak<RefCell<CanvasView>>,
    /// Frames drawn (or attempted, before the surface exists).
    #[cfg(test)]
    frames: u32,
}

struct Binding {
    visual: IDCompositionVisual,
    composed: Arc<dyn ComposedTarget>,
    size: (u32, u32),
    scale: f32,
}

impl CanvasView {
    fn new(sizing: SizeReporter, schedule: ScheduleFrame) -> Rc<RefCell<Self>> {
        Rc::new_cyclic(|me| {
            RefCell::new(CanvasView {
                scene: Scene::default(),
                textures: Vec::new(),
                texture_subs: Vec::new(),
                sizing,
                binding: None,
                surface: None,
                dirty: false,
                requested: false,
                frame: None,
                schedule,
                me: me.clone(),
                #[cfg(test)]
                frames: 0,
            })
        })
    }

    /// Schedule a frame to draw the installed scene, unless one is already
    /// pending. Called outside the view's borrow: without a scheduler the
    /// frame runs synchronously, inside this call.
    fn request_frame(view: &Rc<RefCell<CanvasView>>) {
        let (schedule, me) = {
            let mut v = view.borrow_mut();
            if !v.dirty || v.requested {
                return;
            }
            v.requested = true;
            (v.schedule, v.me.clone())
        };
        let task = schedule(Box::new(move || {
            let Some(view) = me.upgrade() else { return };
            let mut v = view.borrow_mut();
            v.requested = false;
            v.frame = None;
            v.render();
        }));
        let mut v = view.borrow_mut();
        // A synchronous frame already ran and cleared the request.
        if v.requested {
            v.frame = task;
        }
    }

    /// The paint effect's write: runs inside the effect, so the layer
    /// subscription sync's reads of each stream source are tracked (a camera
    /// opening or closing re-runs the effect and resubscribes).
    fn install(&mut self, scene: Scene, textures: Vec<TextureLayer>) {
        canvas_core::sync_layer_subscriptions(&textures, &mut self.texture_subs);
        self.scene = scene;
        self.textures = textures;
        self.dirty = true;
    }

    fn ready(&mut self, ev: OnReadyEvent) {
        let GraphicsTarget::RawWindow(surface) = ev.target else { return };
        let Some(composed) = surface.composed_target().cloned() else {
            eprintln!("[canvas-native] Windows canvas: the graphics surface is not a composition visual");
            return;
        };
        // SAFETY: the provider holds the visual for the surface's lifetime;
        // `cloned` adds our own reference.
        let Some(visual) = (unsafe { IDCompositionVisual::from_raw_borrowed(&composed.visual().as_ptr()) }).cloned()
        else {
            return;
        };
        self.binding = Some(Binding { visual, composed, size: ev.size, scale: ev.scale });
        self.surface = None;
        self.report_size();
        self.render();
    }

    fn resize(&mut self, ev: OnResizeEvent) {
        let Some(binding) = &mut self.binding else { return };
        binding.size = ev.size;
        binding.scale = ev.scale;
        if let Some(s) = &mut self.surface {
            if s.resize(ev.size).is_err() {
                self.surface = None;
            }
        }
        self.report_size();
        self.render();
    }

    fn lost(&mut self) {
        self.surface = None;
        self.binding = None;
    }

    fn report_size(&self) {
        if let Some(b) = &self.binding {
            let s = b.scale.max(0.01);
            self.sizing.report(b.size.0 as f32 / s, b.size.1 as f32 / s);
        }
    }

    fn render(&mut self) {
        #[cfg(test)]
        {
            self.frames += 1;
        }
        self.dirty = false;
        let Some(binding) = &self.binding else { return };
        if self.surface.is_none() {
            match WindowSurface::new(binding) {
                Ok(s) => self.surface = Some(s),
                Err(e) => {
                    eprintln!("[canvas-native] Windows canvas: surface creation failed: {e}");
                    return;
                }
            }
        }
        let Some(surface) = &mut self.surface else { return };
        if let Err(e) = surface.frame(&self.scene, &self.textures, binding.scale) {
            // A lost device (driver reset, GPU removed): drop every
            // resource built on it; the next render rebuilds on a new one.
            eprintln!("[canvas-native] Windows canvas: frame failed ({e}); rebuilding the device");
            self.surface = None;
            reset_gpu();
        }
    }
}

// ============================================================================
// Device
// ============================================================================

/// The Direct3D 11 + Direct2D device every canvas on the UI thread shares.
struct Gpu {
    d3d: ID3D11Device,
    factory: ID2D1Factory1,
    device: ID2D1Device,
}

thread_local! {
    static GPU: RefCell<Option<Rc<Gpu>>> = const { RefCell::new(None) };
}

fn gpu() -> Result<Rc<Gpu>> {
    if let Some(g) = GPU.with(|g| g.borrow().clone()) {
        return Ok(g);
    }
    let g = Rc::new(unsafe { create_gpu()? });
    GPU.with(|slot| *slot.borrow_mut() = Some(g.clone()));
    Ok(g)
}

fn reset_gpu() {
    GPU.with(|g| *g.borrow_mut() = None);
}

unsafe fn create_gpu() -> Result<Gpu> {
    // A hardware device first (which is also what a VM's basic display
    // adapter offers); WARP if there is none at all.
    let mut d3d: Option<ID3D11Device> = None;
    let mut created = Err(windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL));
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        created = D3D11CreateDevice(
            None,
            driver,
            None,
            D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut d3d),
            None,
            None,
        );
        if created.is_ok() {
            break;
        }
    }
    created?;
    let d3d = d3d.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
    let dxgi: IDXGIDevice = d3d.cast()?;
    let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
    let device = factory.CreateDevice(&dxgi)?;
    Ok(Gpu { d3d, factory, device })
}

fn bitmap_props(opts: D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        // 96 DPI = one DIP per pixel: the replay applies the scale itself.
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: opts,
        colorContext: std::mem::ManuallyDrop::new(None),
    }
}

// ============================================================================
// Window surface
// ============================================================================

/// The swapchain on the canvas's visual, the offscreen frame it shows, and
/// the painter that draws the frame.
struct WindowSurface {
    swap: IDXGISwapChain1,
    /// The swapchain's back buffer as a Direct2D target; rebuilt on resize.
    back: Option<ID2D1Bitmap1>,
    frame: ID2D1Bitmap1,
    size: (u32, u32),
    painter: Painter,
}

impl WindowSurface {
    fn new(binding: &Binding) -> Result<Self> {
        let gpu = gpu()?;
        let size = (binding.size.0.max(1), binding.size.1.max(1));
        unsafe {
            let dxgi: IDXGIDevice = gpu.d3d.cast()?;
            let factory: IDXGIFactory2 = dxgi.GetAdapter()?.GetParent()?;
            let desc = DXGI_SWAP_CHAIN_DESC1 {
                Width: size.0,
                Height: size.1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
                BufferCount: 2,
                Scaling: DXGI_SCALING_STRETCH,
                SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
                // A canvas is transparent where nothing is drawn; composition
                // swapchains take premultiplied alpha (never straight).
                AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
                ..Default::default()
            };
            let swap = factory.CreateSwapChainForComposition(&gpu.d3d, &desc, None)?;
            binding.visual.SetContent(&swap)?;
            // The new content appears only at the next commit of the tree.
            binding.composed.commit();
            let painter = Painter::new(gpu)?;
            let frame = painter.target_bitmap(size)?;
            Ok(WindowSurface { swap, back: None, frame, size, painter })
        }
    }

    fn resize(&mut self, size: (u32, u32)) -> Result<()> {
        let size = (size.0.max(1), size.1.max(1));
        if size == self.size {
            return Ok(());
        }
        self.back = None;
        unsafe {
            self.painter.main().SetTarget(None);
            self.swap.ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
        }
        self.frame = self.painter.target_bitmap(size)?;
        self.size = size;
        Ok(())
    }

    fn frame(&mut self, scene: &Scene, textures: &[TextureLayer], scale: f32) -> Result<()> {
        self.painter.render(&self.frame, self.size, scale, scene.ops(), textures)?;
        unsafe {
            if self.back.is_none() {
                // Flip-model buffer 0 is always the current back buffer.
                let surface: IDXGISurface = self.swap.GetBuffer(0)?;
                self.back = Some(self.painter.main().CreateBitmapFromDxgiSurface(
                    &surface,
                    Some(&bitmap_props(D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW)),
                )?);
            }
            let ctx = self.painter.main();
            let back: ID2D1Image = self.back.as_ref().expect("just created").cast()?;
            ctx.SetTarget(&back);
            ctx.BeginDraw();
            ctx.SetTransform(&Matrix3x2::identity());
            ctx.DrawImage(&self.frame, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_COPY);
            let r = ctx.EndDraw(None, None);
            ctx.SetTarget(None);
            r?;
            // Not vsync-blocking: the UI thread presents every canvas, and the
            // compositor shows the latest frame at its own cadence.
            self.swap.Present(0, DXGI_PRESENT(0)).ok()?;
        }
        Ok(())
    }
}

// ============================================================================
// Painter
// ============================================================================

/// Affine `(a, b, c, d, e, f)` in Canvas2D order: `x' = a·x + c·y + e`,
/// `y' = b·x + d·y + f` — Direct2D's `Matrix3x2` field for field.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Aff {
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    e: f32,
    f: f32,
}

impl Aff {
    const IDENTITY: Aff = Aff { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    fn scale(s: f32) -> Aff {
        Aff { a: s, d: s, ..Aff::IDENTITY }
    }

    fn of(t: &Transform) -> Aff {
        Aff { a: t.a, b: t.b, c: t.c, d: t.d, e: t.e, f: t.f }
    }

    /// Apply `self` first, then `next`.
    fn then(&self, n: &Aff) -> Aff {
        Aff {
            a: self.a * n.a + self.b * n.c,
            b: self.a * n.b + self.b * n.d,
            c: self.c * n.a + self.d * n.c,
            d: self.c * n.b + self.d * n.d,
            e: self.e * n.a + self.f * n.c + n.e,
            f: self.e * n.b + self.f * n.d + n.f,
        }
    }

    fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }

    fn matrix(&self) -> Matrix3x2 {
        Matrix3x2 { M11: self.a, M12: self.b, M21: self.c, M22: self.d, M31: self.e, M32: self.f }
    }
}

/// One clip in force: its geometry in the user space of the `Clip` op, and
/// — when it is an axis-aligned rectangle under an axis-aligned transform —
/// its device-space rectangle, which can be a real clip on the fast path.
#[derive(Clone)]
struct ClipEntry {
    geometry: ID2D1Geometry,
    transform: Aff,
    rect: Option<(f32, f32, f32, f32)>,
}

/// The `Save`/`Restore` state: transform and clips.
#[derive(Clone)]
struct State {
    ctm: Aff,
    clips: Vec<ClipEntry>,
}

/// Device resources for one canvas: a device context per nesting depth
/// (layer bakes and scratch draws use the next depth), the persistent
/// layers, and the image / texture uploads.
struct Painter {
    gpu: Rc<Gpu>,
    /// `ctxs[d]` draws depth `d`; created on first use.
    ctxs: Vec<ID2D1DeviceContext>,
    /// The target each depth is drawing into (blend modes read it back).
    targets: Vec<Option<ID2D1Bitmap1>>,
    brush: ID2D1SolidColorBrush,
    /// Device size the persistent bitmaps below were made for.
    size: (u32, u32),
    layers: HashMap<u32, ID2D1Bitmap1>,
    cached: HashMap<u32, ID2D1Bitmap1>,
    /// Scratch bitmaps by `(depth, slot)`.
    scratch: HashMap<(usize, u8), ID2D1Bitmap1>,
    /// Uploaded `Image` sources by id, with the generation uploaded.
    images: HashMap<u64, (u64, ID2D1Bitmap1)>,
    /// Texture-layer uploads by layer index, with their pixel size.
    textures: HashMap<u32, ((u32, u32), ID2D1Bitmap1)>,
}

const SLOT_DRAW: u8 = 0;
const SLOT_SNAPSHOT: u8 = 1;
const SLOT_MASK_CONTENT: u8 = 2;
const SLOT_MASK: u8 = 3;

fn transparent() -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }
}

fn color(c: Color, alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: c.r as f32 / 255.0,
        g: c.g as f32 / 255.0,
        b: c.b as f32 / 255.0,
        a: c.a as f32 / 255.0 * alpha,
    }
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

impl Painter {
    fn new(gpu: Rc<Gpu>) -> Result<Self> {
        unsafe {
            let ctx = gpu.device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            let brush = ctx.CreateSolidColorBrush(&transparent(), None)?;
            Ok(Painter {
                gpu,
                ctxs: vec![ctx],
                targets: vec![None],
                brush,
                size: (0, 0),
                layers: HashMap::new(),
                cached: HashMap::new(),
                scratch: HashMap::new(),
                images: HashMap::new(),
                textures: HashMap::new(),
            })
        }
    }

    fn main(&self) -> &ID2D1DeviceContext {
        &self.ctxs[0]
    }

    fn ctx(&mut self, depth: usize) -> Result<ID2D1DeviceContext> {
        while self.ctxs.len() <= depth {
            let c = unsafe { self.gpu.device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)? };
            self.ctxs.push(c);
            self.targets.push(None);
        }
        Ok(self.ctxs[depth].clone())
    }

    fn target_bitmap(&self, size: (u32, u32)) -> Result<ID2D1Bitmap1> {
        unsafe {
            self.main().CreateBitmap(
                D2D_SIZE_U { width: size.0.max(1), height: size.1.max(1) },
                None,
                0,
                &bitmap_props(D2D1_BITMAP_OPTIONS_TARGET),
            )
        }
    }

    fn scratch(&mut self, depth: usize, slot: u8) -> Result<ID2D1Bitmap1> {
        if let Some(b) = self.scratch.get(&(depth, slot)) {
            return Ok(b.clone());
        }
        let b = self.target_bitmap(self.size)?;
        self.scratch.insert((depth, slot), b.clone());
        Ok(b)
    }

    /// Draw `ops` into `target` (device-sized `size`), cleared to transparent
    /// first, under a `scale` base transform. `textures` are the canvas's
    /// texture layers, composited where a `Texture` op names them.
    fn render(
        &mut self,
        target: &ID2D1Bitmap1,
        size: (u32, u32),
        scale: f32,
        ops: &[DrawOp],
        textures: &[TextureLayer],
    ) -> Result<()> {
        if size != self.size {
            // Persistent rasters are device-sized: a size change rebuilds them
            // (a `LayerCached` re-bakes on its next sight).
            self.layers.clear();
            self.cached.clear();
            self.scratch.clear();
            self.size = size;
        }
        let base = State { ctm: Aff::scale(scale), clips: Vec::new() };
        self.begin(0, target, true)?;
        self.replay(0, ops, &base, scale, textures);
        self.end(0)
    }

    fn begin(&mut self, depth: usize, target: &ID2D1Bitmap1, clear: bool) -> Result<()> {
        let ctx = self.ctx(depth)?;
        self.targets[depth] = Some(target.clone());
        unsafe {
            ctx.SetTarget(target);
            ctx.BeginDraw();
            ctx.SetTransform(&Matrix3x2::identity());
            if clear {
                ctx.Clear(Some(&transparent()));
            }
        }
        Ok(())
    }

    fn end(&mut self, depth: usize) -> Result<()> {
        let ctx = self.ctxs[depth].clone();
        self.targets[depth] = None;
        unsafe {
            let r = ctx.EndDraw(None, None);
            ctx.SetTarget(None);
            r
        }
    }

    fn replay(&mut self, depth: usize, ops: &[DrawOp], start: &State, scale: f32, textures: &[TextureLayer]) {
        let mut state = start.clone();
        let mut saved: Vec<State> = Vec::new();
        for op in ops {
            match op {
                DrawOp::Fill { path, paint, fill_rule } => {
                    let Some(geometry) = self.geometry(path, *fill_rule) else { continue };
                    let Some(brush) = self.brush(paint) else { continue };
                    self.draw(depth, &state, paint.blend, 1.0, |c, _| unsafe {
                        c.FillGeometry(&geometry, &brush, None);
                    });
                }
                DrawOp::Stroke { path, paint, stroke } => {
                    if stroke.width <= 0.0 {
                        continue;
                    }
                    let Some(geometry) = self.geometry(path, FillRule::NonZero) else { continue };
                    let Some(brush) = self.brush(paint) else { continue };
                    let style = self.stroke_style(stroke);
                    let width = stroke.width;
                    self.draw(depth, &state, paint.blend, 1.0, |c, _| unsafe {
                        c.DrawGeometry(&geometry, &brush, width, style.as_ref());
                    });
                }
                DrawOp::Save => saved.push(state.clone()),
                DrawOp::Restore => {
                    if let Some(s) = saved.pop() {
                        state = s;
                    }
                }
                DrawOp::Transform(t) => state.ctm = Aff::of(t).then(&state.ctm),
                DrawOp::Clip { path, fill_rule } => {
                    if let Some(geometry) = self.geometry(path, *fill_rule) {
                        let rect = axis_aligned_rect(path, &state.ctm);
                        state.clips.push(ClipEntry { geometry, transform: state.ctm, rect });
                    }
                }
                DrawOp::Image { image, dst, alpha, blend } => {
                    let Some(bitmap) = self.image(image) else { continue };
                    let dest = D2D_RECT_F { left: dst.x, top: dst.y, right: dst.x + dst.w, bottom: dst.y + dst.h };
                    self.draw(depth, &state, *blend, *alpha, |c, a| unsafe {
                        c.DrawBitmap(&bitmap, Some(&dest), a, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
                    });
                }
                DrawOp::Layer { id, clear, ops, alpha, blend } => {
                    let fresh = !self.layers.contains_key(id);
                    let Ok(bitmap) = self.persistent(*id, false) else { continue };
                    // The layer's content shares the canvas's coordinates: bake
                    // under the current transform, no clips (clips apply when
                    // it is composited).
                    let bake = State { ctm: state.ctm, clips: Vec::new() };
                    if self.begin(depth + 1, &bitmap, *clear || fresh).is_ok() {
                        self.replay(depth + 1, ops, &bake, scale, &[]);
                        let _ = self.end(depth + 1);
                    }
                    self.composite_bitmap(depth, &state, &bitmap, None, *alpha, *blend);
                }
                DrawOp::LayerCached { id, dirty, transform, ops, alpha, blend } => {
                    let fresh = !self.cached.contains_key(id);
                    let Ok(bitmap) = self.persistent(*id, true) else { continue };
                    if *dirty || fresh {
                        let bake = State { ctm: Aff::scale(scale), clips: Vec::new() };
                        if self.begin(depth + 1, &bitmap, true).is_ok() {
                            self.replay(depth + 1, ops, &bake, scale, &[]);
                            let _ = self.end(depth + 1);
                        }
                    }
                    // The logical transform conjugated into device space:
                    // `scale⁻¹ · T · scale` = T with its translation scaled.
                    let t = Aff::of(transform);
                    let device = Aff { e: t.e * scale, f: t.f * scale, ..t };
                    self.composite_bitmap(depth, &state, &bitmap, Some(device), *alpha, *blend);
                }
                DrawOp::Shapes { shapes, blend } => {
                    // No instanced path: one Fill per shape, in order, so a
                    // batched shape and a hand-written fill draw the same.
                    let fills: Vec<DrawOp> = shapes.iter().map(|s| s.to_fill_op(*blend)).collect();
                    self.replay_in(depth, &fills, &mut state, scale);
                }
                DrawOp::Glyphs { font, glyphs, paint } => {
                    let expanded = canvas_core::expand_glyph_run(font, glyphs, paint);
                    self.replay_in(depth, &expanded, &mut state, scale);
                }
                DrawOp::Texture { index } => {
                    // `place_textures` guarantees the base state here; a nested
                    // replay passes no textures.
                    if let Some(layer) = textures.get(*index as usize) {
                        self.texture(depth, *index, layer, scale);
                    }
                }
                DrawOp::MaskGroup { content, mask, luminance, alpha, blend } => {
                    self.mask_group(depth, &state, content, mask, *luminance, *alpha, *blend, scale);
                }
                // `DrawOp` is `#[non_exhaustive]`: an op this renderer does not
                // know no-ops rather than abort the frame.
                _ => {}
            }
        }
    }

    /// Replay an expansion (`Shapes`, `Glyphs`) in the CURRENT state. The
    /// expansions balance their own Save/Restore, so `state` comes back as
    /// it went in.
    fn replay_in(&mut self, depth: usize, ops: &[DrawOp], state: &mut State, scale: f32) {
        self.replay(depth, ops, state, scale, &[]);
    }

    /// Draw one primitive with `blend` and `alpha` under the clips in force.
    /// `f` draws onto the context it is given, at the alpha it is given; the
    /// context's transform is the state's when it runs.
    fn draw(&mut self, depth: usize, state: &State, blend: BlendMode, alpha: f32, f: impl Fn(&ID2D1DeviceContext, f32)) {
        let alpha = alpha.clamp(0.0, 1.0);
        let rects_only = state.clips.iter().all(|c| c.rect.is_some());
        if matches!(blend, BlendMode::Normal) && rects_only {
            let ctx = self.ctxs[depth].clone();
            unsafe {
                let clip = clip_rect_intersection(&state.clips);
                if let Some(r) = clip {
                    if r.0 >= r.2 || r.1 >= r.3 {
                        return;
                    }
                    ctx.SetTransform(&Matrix3x2::identity());
                    ctx.PushAxisAlignedClip(
                        &D2D_RECT_F { left: r.0, top: r.1, right: r.2, bottom: r.3 },
                        D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    );
                }
                ctx.SetTransform(&state.ctm.matrix());
                f(&ctx, alpha);
                if clip.is_some() {
                    ctx.PopAxisAlignedClip();
                }
            }
            return;
        }
        // Isolated: draw into scratch with the clips applied there, then
        // composite with the blend.
        let Ok(scratch) = self.scratch(depth + 1, SLOT_DRAW) else { return };
        if self.begin(depth + 1, &scratch, true).is_err() {
            return;
        }
        let c = self.ctxs[depth + 1].clone();
        let pushed = unsafe { push_clips(&c, &state.clips) };
        unsafe {
            c.SetTransform(&state.ctm.matrix());
            f(&c, 1.0);
            pop_clips(&c, pushed);
        }
        if self.end(depth + 1).is_err() {
            return;
        }
        self.composite(depth, &scratch, alpha, blend);
    }

    /// Composite a persistent layer bitmap (device-sized; drawn at
    /// `device_transform`, identity when `None`) under the state's clips.
    fn composite_bitmap(
        &mut self,
        depth: usize,
        state: &State,
        bitmap: &ID2D1Bitmap1,
        device_transform: Option<Aff>,
        alpha: f32,
        blend: BlendMode,
    ) {
        let xf = device_transform.unwrap_or(Aff::IDENTITY);
        // The layer is already in device space: draw it with the
        // device transform, not the state's.
        let at_device = State { ctm: Aff::IDENTITY, clips: state.clips.clone() };
        self.draw(depth, &at_device, blend, alpha, |c, a| unsafe {
            c.SetTransform(&xf.matrix());
            c.DrawBitmap(bitmap, None, a, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
        });
    }

    /// Composite a device-space image onto depth `depth`'s target with
    /// `blend` (no clips: callers applied them when drawing `image`).
    fn composite(&mut self, depth: usize, image: &ID2D1Bitmap1, alpha: f32, blend: BlendMode) {
        let ctx = self.ctxs[depth].clone();
        unsafe {
            ctx.SetTransform(&Matrix3x2::identity());
            let Some(source) = with_opacity(&ctx, image, alpha) else { return };
            let mode = match blend {
                BlendMode::DestinationOut => Some(D2D1_COMPOSITE_MODE_DESTINATION_OUT),
                _ => blend_effect_mode(blend).is_none().then_some(D2D1_COMPOSITE_MODE_SOURCE_OVER),
            };
            if let Some(mode) = mode {
                ctx.DrawImage(&source, None, None, D2D1_INTERPOLATION_MODE_LINEAR, mode);
                return;
            }
            let Some(effect_mode) = blend_effect_mode(blend) else { return };
            // The separable / non-separable modes need the pixels already
            // drawn: blend over a copy of the target, then write the result
            // back (where the source is transparent it equals the copy).
            let Some(target) = self.targets[depth].clone() else { return };
            let Ok(snapshot) = self.scratch(depth, SLOT_SNAPSHOT) else { return };
            if ctx.Flush(None, None).is_err() || snapshot.CopyFromBitmap(None, &target, None).is_err() {
                return;
            }
            let Ok(effect) = ctx.CreateEffect(&CLSID_D2D1Blend) else { return };
            effect.SetInput(0, &snapshot, true);
            effect.SetInput(1, &source, true);
            let _ = effect.SetValue(D2D1_BLEND_PROP_MODE.0 as u32, D2D1_PROPERTY_TYPE_ENUM, &(effect_mode.0 as u32).to_le_bytes());
            let Ok(out) = effect.GetOutput() else { return };
            ctx.DrawImage(&out, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_COPY);
        }
    }

    fn persistent(&mut self, id: u32, cached: bool) -> Result<ID2D1Bitmap1> {
        let map = if cached { &self.cached } else { &self.layers };
        if let Some(b) = map.get(&id) {
            return Ok(b.clone());
        }
        let b = self.target_bitmap(self.size)?;
        if cached { &mut self.cached } else { &mut self.layers }.insert(id, b.clone());
        Ok(b)
    }

    #[allow(clippy::too_many_arguments)]
    fn mask_group(
        &mut self,
        depth: usize,
        state: &State,
        content: &[DrawOp],
        mask: &[DrawOp],
        luminance: bool,
        alpha: f32,
        blend: BlendMode,
        scale: f32,
    ) {
        // Both halves in the group's coordinates, unclipped (the clips
        // apply to the masked result).
        let bake = State { ctm: state.ctm, clips: Vec::new() };
        let (Ok(content_bmp), Ok(mask_bmp)) =
            (self.scratch(depth + 1, SLOT_MASK_CONTENT), self.scratch(depth + 1, SLOT_MASK))
        else {
            return;
        };
        for (bmp, ops) in [(&content_bmp, content), (&mask_bmp, mask)] {
            if self.begin(depth + 1, bmp, true).is_err() {
                return;
            }
            self.replay(depth + 1, ops, &bake, scale, &[]);
            if self.end(depth + 1).is_err() {
                return;
            }
        }
        let at_device = State { ctm: Aff::IDENTITY, clips: state.clips.clone() };
        self.draw(depth, &at_device, blend, alpha, |c, a| unsafe {
            let Ok(masked) = c.CreateEffect(&CLSID_D2D1AlphaMask) else { return };
            masked.SetInput(0, &content_bmp, true);
            if luminance {
                let Ok(luma) = c.CreateEffect(&CLSID_D2D1LuminanceToAlpha) else { return };
                luma.SetInput(0, &mask_bmp, true);
                let Ok(l) = luma.GetOutput() else { return };
                masked.SetInput(1, &l, true);
            } else {
                masked.SetInput(1, &mask_bmp, true);
            }
            let Ok(out) = masked.GetOutput() else { return };
            let Some(img) = with_opacity_image(c, &out, a) else { return };
            c.SetTransform(&Matrix3x2::identity());
            c.DrawImage(&img, None, None, D2D1_INTERPOLATION_MODE_LINEAR, D2D1_COMPOSITE_MODE_SOURCE_OVER);
        });
    }

    /// Composite texture layer `index` (base state): its latest frame,
    /// cropped and fitted into its rect, clipped to that rect rounded by the
    /// corner radius, faded by its opacity, with its border frame — the same
    /// geometry as every other renderer.
    fn texture(&mut self, depth: usize, index: u32, layer: &TextureLayer, scale: f32) {
        let mut rgba = Vec::new();
        let Some((vw, vh)) = layer.resolve_rgba(&mut rgba) else { return };
        if vw == 0 || vh == 0 || rgba.len() < vw as usize * vh as usize * 4 {
            return;
        }
        let (_, _, dw, dh) = (layer.rect)();
        if dw < 1.0 || dh < 1.0 {
            return;
        }
        let ((sx, sy, sw, sh), (ox, oy, ow, oh)) = layer.source_rects(vw as f32, vh as f32);
        if sw <= 0.0 || sh <= 0.0 || ow <= 0.0 || oh <= 0.0 {
            return;
        }
        let Some(bitmap) = self.texture_bitmap(index, vw, vh, &rgba) else { return };
        let r = (layer.corner_radius)().clamp(0.0, ow.min(oh) / 2.0);
        let opacity = layer.opacity.clamp(0.0, 1.0);
        let Some((kx, ky, tx, ty)) = layer_geometry::source_to_dest((sx, sy, sw, sh), (ox, oy, ow, oh)) else {
            return;
        };
        let dest = D2D_RECT_F { left: tx, top: ty, right: tx + vw as f32 * kx, bottom: ty + vh as f32 * ky };
        let base = Aff::scale(scale);
        let mut state = State { ctm: base, clips: Vec::new() };
        if let Some(clip) = self.rounded_rect(ox, oy, ow, oh, r) {
            let rect = (r < 0.5).then(|| {
                let (x0, y0) = base.apply(ox, oy);
                let (x1, y1) = base.apply(ox + ow, oy + oh);
                (x0, y0, x1, y1)
            });
            state.clips.push(ClipEntry { geometry: clip, transform: base, rect });
        }
        self.draw(depth, &state, BlendMode::Normal, opacity, |c, a| unsafe {
            c.DrawBitmap(&bitmap, Some(&dest), a, D2D1_INTERPOLATION_MODE_LINEAR, None, None);
        });
        let bw = layer.border_width;
        if bw > 0.0 {
            let inset = bw / 2.0;
            let (Some(frame), Ok(brush)) = (
                self.rounded_rect(ox + inset, oy + inset, ow - bw, oh - bw, (r - inset).max(0.0)),
                unsafe { self.main().CreateSolidColorBrush(&color(layer.border_color, opacity), None) },
            ) else {
                return;
            };
            self.draw(depth, &state, BlendMode::Normal, 1.0, |c, _| unsafe {
                c.DrawGeometry(&frame, &brush, bw, None);
            });
        }
    }

    fn texture_bitmap(&mut self, index: u32, w: u32, h: u32, rgba: &[u8]) -> Option<ID2D1Bitmap1> {
        let bgra = pixels::rgba8_to_bgra8_premultiplied(&rgba[..w as usize * h as usize * 4]);
        if let Some(((cw, ch), b)) = self.textures.get(&index) {
            if (*cw, *ch) == (w, h) {
                unsafe { b.CopyFromMemory(None, bgra.as_ptr() as *const _, w * 4).ok()? };
                return Some(b.clone());
            }
        }
        let b = self.upload(w, h, &bgra)?;
        self.textures.insert(index, ((w, h), b.clone()));
        Some(b)
    }

    fn image(&mut self, image: &Arc<ImageSource>) -> Option<ID2D1Bitmap1> {
        if !image.is_valid() {
            return None;
        }
        if let Some((generation, b)) = self.images.get(&image.id) {
            if *generation == image.generation {
                return Some(b.clone());
            }
        }
        let bgra = pixels::rgba8_to_bgra8_premultiplied(&image.rgba);
        let b = self.upload(image.width, image.height, &bgra)?;
        self.images.insert(image.id, (image.generation, b.clone()));
        Some(b)
    }

    fn upload(&self, w: u32, h: u32, bgra: &[u8]) -> Option<ID2D1Bitmap1> {
        unsafe {
            self.main()
                .CreateBitmap(
                    D2D_SIZE_U { width: w, height: h },
                    Some(bgra.as_ptr() as *const _),
                    w * 4,
                    &bitmap_props(D2D1_BITMAP_OPTIONS(0)),
                )
                .ok()
        }
    }

    fn rounded_rect(&self, x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<ID2D1Geometry> {
        let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }, radiusX: r, radiusY: r };
        unsafe { self.gpu.factory.CreateRoundedRectangleGeometry(&rr).ok()?.cast().ok() }
    }

    /// `path` as a Direct2D geometry, Canvas2D path semantics: a drawing
    /// command with no current point starts a subpath there, a `MoveTo` ends
    /// the open one, and after `Close` the current point is the closed
    /// subpath's start. `None` for an empty path.
    fn geometry(&self, path: &Path, rule: FillRule) -> Option<ID2D1Geometry> {
        if path.segs.is_empty() {
            return None;
        }
        unsafe {
            let geometry = self.gpu.factory.CreatePathGeometry().ok()?;
            let sink = geometry.Open().ok()?;
            sink.SetFillMode(match rule {
                FillRule::EvenOdd => D2D1_FILL_MODE_ALTERNATE,
                _ => D2D1_FILL_MODE_WINDING,
            });
            let mut open = false;
            let mut current: Option<(f32, f32)> = None;
            let mut start = (0.0, 0.0);
            let begin = |sink: &windows::Win32::Graphics::Direct2D::ID2D1GeometrySink,
                             open: &mut bool,
                             start: &mut (f32, f32),
                             p: (f32, f32)| {
                if *open {
                    sink.EndFigure(D2D1_FIGURE_END_OPEN);
                }
                sink.BeginFigure(pt(p.0, p.1), D2D1_FIGURE_BEGIN_FILLED);
                *open = true;
                *start = p;
            };
            for seg in &path.segs {
                match *seg {
                    PathSeg::MoveTo { x, y } => {
                        begin(&sink, &mut open, &mut start, (x, y));
                        current = Some((x, y));
                    }
                    PathSeg::LineTo { x, y } => {
                        if !open {
                            begin(&sink, &mut open, &mut start, current.unwrap_or((x, y)));
                        }
                        sink.AddLine(pt(x, y));
                        current = Some((x, y));
                    }
                    PathSeg::QuadTo { cx, cy, x, y } => {
                        if !open {
                            begin(&sink, &mut open, &mut start, current.unwrap_or((cx, cy)));
                        }
                        sink.AddQuadraticBezier(&D2D1_QUADRATIC_BEZIER_SEGMENT { point1: pt(cx, cy), point2: pt(x, y) });
                        current = Some((x, y));
                    }
                    PathSeg::CubicTo { c1x, c1y, c2x, c2y, x, y } => {
                        if !open {
                            begin(&sink, &mut open, &mut start, current.unwrap_or((c1x, c1y)));
                        }
                        sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                            point1: pt(c1x, c1y),
                            point2: pt(c2x, c2y),
                            point3: pt(x, y),
                        });
                        current = Some((x, y));
                    }
                    PathSeg::Close => {
                        if open {
                            sink.EndFigure(D2D1_FIGURE_END_CLOSED);
                            open = false;
                            current = Some(start);
                        }
                    }
                }
            }
            if open {
                sink.EndFigure(D2D1_FIGURE_END_OPEN);
            }
            sink.Close().ok()?;
            geometry.cast().ok()
        }
    }

    /// The paint as a brush in user space; `None` paints nothing (a
    /// degenerate gradient, as on Canvas2D).
    fn brush(&self, paint: &Paint) -> Option<ID2D1Brush> {
        unsafe {
            match &paint.kind {
                PaintKind::Solid(c) => {
                    // A brush per op: a shared brush recoloured in place would
                    // change ops already batched with it.
                    let b = self.main().CreateSolidColorBrush(&color(*c, 1.0), None).ok()?;
                    b.cast().ok()
                }
                PaintKind::Linear(g) => {
                    if g.x0 == g.x1 && g.y0 == g.y1 {
                        return None;
                    }
                    let (rt, stops) = self.stops(&g.stops)?;
                    let b = rt
                        .CreateLinearGradientBrush(
                            &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(g.x0, g.y0), endPoint: pt(g.x1, g.y1) },
                            None,
                            &stops,
                        )
                        .ok()?;
                    b.cast().ok()
                }
                PaintKind::Radial(g) => {
                    if g.r <= 0.0 {
                        return None;
                    }
                    let (rt, stops) = self.stops(&g.stops)?;
                    let b = rt
                        .CreateRadialGradientBrush(
                            &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                                center: pt(g.cx, g.cy),
                                gradientOriginOffset: pt(0.0, 0.0),
                                radiusX: g.r,
                                radiusY: g.r,
                            },
                            None,
                            &stops,
                        )
                        .ok()?;
                    b.cast().ok()
                }
                // `PaintKind` is non-exhaustive; an unknown paint draws
                // nothing rather than a wrong colour.
                _ => None,
            }
        }
    }

    fn stops(
        &self,
        stops: &[canvas_core::GradientStop],
    ) -> Option<(ID2D1RenderTarget, windows::Win32::Graphics::Direct2D::ID2D1GradientStopCollection)> {
        if stops.is_empty() {
            return None;
        }
        let mut d: Vec<D2D1_GRADIENT_STOP> = stops
            .iter()
            .map(|s| D2D1_GRADIENT_STOP { position: s.offset.clamp(0.0, 1.0), color: color(s.color, 1.0) })
            .collect();
        d.sort_by(|a, b| a.position.partial_cmp(&b.position).unwrap_or(std::cmp::Ordering::Equal));
        unsafe {
            let rt: ID2D1RenderTarget = self.main().cast().ok()?;
            // Pad beyond the ends (Canvas2D), interpolated in sRGB.
            let collection = rt.CreateGradientStopCollection(&d, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()?;
            Some((rt, collection))
        }
    }

    fn stroke_style(&self, stroke: &Stroke) -> Option<ID2D1StrokeStyle> {
        let cap = cap_style(stroke.cap);
        let join = match stroke.join {
            LineJoin::Round => D2D1_LINE_JOIN_ROUND,
            LineJoin::Bevel => D2D1_LINE_JOIN_BEVEL,
            // Canvas2D miters up to the limit, then bevels.
            _ => D2D1_LINE_JOIN_MITER_OR_BEVEL,
        };
        let dashes = dash_pattern(&stroke.dash, stroke.width);
        let props = D2D1_STROKE_STYLE_PROPERTIES {
            startCap: cap,
            endCap: cap,
            dashCap: cap,
            lineJoin: join,
            miterLimit: stroke.miter_limit.max(1.0),
            dashStyle: if dashes.is_some() { D2D1_DASH_STYLE_CUSTOM } else { D2D1_DASH_STYLE_SOLID },
            // Direct2D measures dashes in stroke widths.
            dashOffset: if dashes.is_some() { stroke.dash_offset / stroke.width } else { 0.0 },
        };
        unsafe {
            let factory: ID2D1Factory = self.gpu.factory.cast().ok()?;
            factory.CreateStrokeStyle(&props, dashes.as_deref()).ok()
        }
    }
}

fn cap_style(cap: LineCap) -> D2D1_CAP_STYLE {
    match cap {
        LineCap::Round => D2D1_CAP_STYLE_ROUND,
        LineCap::Square => D2D1_CAP_STYLE_SQUARE,
        _ => D2D1_CAP_STYLE_FLAT,
    }
}

/// Canvas2D dash lengths (logical px) as Direct2D's (stroke widths). An odd
/// list repeats to even, as Canvas2D does; an empty, all-zero or negative
/// list is a solid line.
fn dash_pattern(dash: &[f32], width: f32) -> Option<Vec<f32>> {
    if dash.is_empty() || width <= 0.0 || dash.iter().any(|d| *d < 0.0 || !d.is_finite()) || dash.iter().all(|d| *d == 0.0) {
        return None;
    }
    let mut d: Vec<f32> = dash.iter().map(|v| v / width).collect();
    if d.len() % 2 == 1 {
        d.extend_from_within(..);
    }
    Some(d)
}

/// `BlendMode` → the Blend effect's mode; `None` for the modes drawn with a
/// composite mode instead (`Normal`, `DestinationOut`, unknown ones).
fn blend_effect_mode(blend: BlendMode) -> Option<D2D1_BLEND_MODE> {
    Some(match blend {
        BlendMode::Multiply => D2D1_BLEND_MODE_MULTIPLY,
        BlendMode::Screen => D2D1_BLEND_MODE_SCREEN,
        BlendMode::Overlay => D2D1_BLEND_MODE_OVERLAY,
        BlendMode::Darken => D2D1_BLEND_MODE_DARKEN,
        BlendMode::Lighten => D2D1_BLEND_MODE_LIGHTEN,
        BlendMode::ColorDodge => D2D1_BLEND_MODE_COLOR_DODGE,
        BlendMode::ColorBurn => D2D1_BLEND_MODE_COLOR_BURN,
        BlendMode::HardLight => D2D1_BLEND_MODE_HARD_LIGHT,
        BlendMode::SoftLight => D2D1_BLEND_MODE_SOFT_LIGHT,
        BlendMode::Difference => D2D1_BLEND_MODE_DIFFERENCE,
        BlendMode::Exclusion => D2D1_BLEND_MODE_EXCLUSION,
        BlendMode::Hue => D2D1_BLEND_MODE_HUE,
        BlendMode::Saturation => D2D1_BLEND_MODE_SATURATION,
        BlendMode::Color => D2D1_BLEND_MODE_COLOR,
        BlendMode::Luminosity => D2D1_BLEND_MODE_LUMINOSITY,
        _ => return None,
    })
}

/// `image` faded by `alpha` (the image itself when opaque).
unsafe fn with_opacity(ctx: &ID2D1DeviceContext, image: &ID2D1Bitmap1, alpha: f32) -> Option<ID2D1Image> {
    with_opacity_image(ctx, &image.cast().ok()?, alpha)
}

unsafe fn with_opacity_image(ctx: &ID2D1DeviceContext, image: &ID2D1Image, alpha: f32) -> Option<ID2D1Image> {
    if alpha >= 1.0 {
        return Some(image.clone());
    }
    let effect = ctx.CreateEffect(&CLSID_D2D1Opacity).ok()?;
    effect.SetInput(0, image, true);
    effect
        .SetValue(D2D1_OPACITY_PROP_OPACITY.0 as u32, D2D1_PROPERTY_TYPE_FLOAT, &alpha.max(0.0).to_le_bytes())
        .ok()?;
    effect.GetOutput().ok()
}

/// Push `clips` onto `ctx` (transform reset per clip). Returns what was
/// pushed, for [`pop_clips`]: `true` = a layer, `false` = an axis-aligned
/// clip.
unsafe fn push_clips(ctx: &ID2D1DeviceContext, clips: &[ClipEntry]) -> Vec<bool> {
    let mut pushed = Vec::with_capacity(clips.len());
    for clip in clips {
        ctx.SetTransform(&Matrix3x2::identity());
        if let Some(r) = clip.rect {
            ctx.PushAxisAlignedClip(&D2D_RECT_F { left: r.0, top: r.1, right: r.2, bottom: r.3 }, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            pushed.push(false);
        } else {
            let params = D2D1_LAYER_PARAMETERS1 {
                contentBounds: D2D_RECT_F { left: f32::MIN, top: f32::MIN, right: f32::MAX, bottom: f32::MAX },
                geometricMask: std::mem::ManuallyDrop::new(Some(clip.geometry.clone())),
                maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                maskTransform: clip.transform.matrix(),
                opacity: 1.0,
                opacityBrush: std::mem::ManuallyDrop::new(None),
                layerOptions: D2D1_LAYER_OPTIONS1_NONE,
            };
            ctx.PushLayer(&params, None);
            std::mem::ManuallyDrop::into_inner(params.geometricMask);
            pushed.push(true);
        }
    }
    pushed
}

unsafe fn pop_clips(ctx: &ID2D1DeviceContext, pushed: Vec<bool>) {
    for layer in pushed.into_iter().rev() {
        if layer {
            ctx.PopLayer();
        } else {
            ctx.PopAxisAlignedClip();
        }
    }
}

/// The device rectangle all `clips` leave (they must all be rectangles);
/// `None` when there are no clips.
fn clip_rect_intersection(clips: &[ClipEntry]) -> Option<(f32, f32, f32, f32)> {
    clips.iter().filter_map(|c| c.rect).fold(None, |acc, r| {
        Some(match acc {
            None => r,
            Some(a) => (a.0.max(r.0), a.1.max(r.1), a.2.min(r.2), a.3.min(r.3)),
        })
    })
}

/// `path`'s device rectangle under `ctm` when the path is one axis-aligned
/// rectangle and `ctm` keeps it axis-aligned (no rotation or skew).
fn axis_aligned_rect(path: &Path, ctm: &Aff) -> Option<(f32, f32, f32, f32)> {
    if ctm.b != 0.0 || ctm.c != 0.0 {
        return None;
    }
    let mut pts: Vec<(f32, f32)> = Vec::new();
    let mut closed = false;
    for (i, seg) in path.segs.iter().enumerate() {
        match *seg {
            PathSeg::MoveTo { x, y } if i == 0 => pts.push((x, y)),
            PathSeg::LineTo { x, y } if i > 0 && !closed => pts.push((x, y)),
            PathSeg::Close if !closed => closed = true,
            _ => return None,
        }
    }
    if pts.len() == 5 && pts[4] == pts[0] {
        pts.pop();
    }
    if pts.len() != 4 {
        return None;
    }
    for i in 0..4 {
        let (a, b) = (pts[i], pts[(i + 1) % 4]);
        if a.0 != b.0 && a.1 != b.1 {
            return None;
        }
    }
    let xs = pts.iter().map(|p| p.0);
    let ys = pts.iter().map(|p| p.1);
    let (x0, x1) = (xs.clone().fold(f32::MAX, f32::min), xs.fold(f32::MIN, f32::max));
    let (y0, y1) = (ys.clone().fold(f32::MAX, f32::min), ys.fold(f32::MIN, f32::max));
    let (dx0, dy0) = ctm.apply(x0, y0);
    let (dx1, dy1) = ctm.apply(x1, y1);
    Some((dx0.min(dx1), dy0.min(dy1), dx0.max(dx1), dy0.max(dy1)))
}

// ============================================================================
// Tests
// ============================================================================

/// Pixel tests: the painter renders scenes into an offscreen bitmap that is
/// read back — the same code path as the window, minus the swapchain copy.
/// They need Direct2D, so they run on Windows (the VM runner in
/// `docs/windows.md`); Wine's layers are stubs, so they skip there.
#[cfg(test)]
mod tests {
    use super::*;
    use canvas_core::{GradientStop, LinearGradient, Rect, ShapeInstance};
    use windows::Win32::Graphics::Direct2D::{D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_MAPPED_RECT, D2D1_MAP_OPTIONS_READ};

    fn under_wine() -> bool {
        use windows::core::s;
        use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
        unsafe {
            GetModuleHandleA(s!("ntdll.dll"))
                .ok()
                .is_some_and(|ntdll| GetProcAddress(ntdll, s!("wine_get_version")).is_some())
        }
    }

    /// Straight RGBA readback of one rendered frame.
    struct Frame {
        w: u32,
        px: Vec<u8>,
    }

    impl Frame {
        /// Premultiplied `[r, g, b, a]` at `(x, y)`.
        fn at(&self, x: u32, y: u32) -> [u8; 4] {
            let i = ((y * self.w + x) * 4) as usize;
            [self.px[i + 2], self.px[i + 1], self.px[i], self.px[i + 3]]
        }
    }

    fn near(a: [u8; 4], b: [u8; 4], tol: i32) -> bool {
        a.iter().zip(b).all(|(x, y)| (*x as i32 - y as i32).abs() <= tol)
    }

    #[track_caller]
    fn assert_px(f: &Frame, x: u32, y: u32, want: [u8; 4]) {
        let got = f.at(x, y);
        assert!(near(got, want, 3), "pixel ({x},{y}) = {got:?}, want ≈{want:?}");
    }

    /// A painter + offscreen frame, rendered and read back on demand. The
    /// painter persists across `render` calls, so persistent layers do.
    struct Harness {
        painter: Painter,
        target: ID2D1Bitmap1,
        size: (u32, u32),
        scale: f32,
    }

    impl Harness {
        fn new(w: u32, h: u32, scale: f32) -> Self {
            let painter = Painter::new(gpu().expect("a Direct2D device")).unwrap();
            let target = painter.target_bitmap((w, h)).unwrap();
            Harness { painter, target, size: (w, h), scale }
        }

        fn render(&mut self, ops: &[DrawOp], textures: &[TextureLayer]) -> Frame {
            self.painter.render(&self.target, self.size, self.scale, ops, textures).unwrap();
            unsafe {
                let cpu = self
                    .painter
                    .main()
                    .CreateBitmap(
                        D2D_SIZE_U { width: self.size.0, height: self.size.1 },
                        None,
                        0,
                        &bitmap_props(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW),
                    )
                    .unwrap();
                cpu.CopyFromBitmap(None, &self.target, None).unwrap();
                let map: D2D1_MAPPED_RECT = cpu.Map(D2D1_MAP_OPTIONS_READ).unwrap();
                let row = self.size.0 as usize * 4;
                let mut px = Vec::with_capacity(row * self.size.1 as usize);
                for y in 0..self.size.1 as usize {
                    px.extend_from_slice(std::slice::from_raw_parts(map.bits.add(y * map.pitch as usize), row));
                }
                cpu.Unmap().unwrap();
                Frame { w: self.size.0, px }
            }
        }
    }

    fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color {
        Color::new(r, g, b, a)
    }

    fn fill(path: Path, c: Color) -> DrawOp {
        DrawOp::Fill { path, paint: Paint::solid(c), fill_rule: FillRule::NonZero }
    }

    const RED: [u8; 4] = [255, 0, 0, 255];
    const BLUE: [u8; 4] = [0, 0, 255, 255];
    const CLEAR: [u8; 4] = [0, 0, 0, 0];

    macro_rules! skip_under_wine {
        () => {
            if under_wine() {
                eprintln!("note: Wine — Direct2D layers/effects are stubs; skipped");
                return;
            }
        };
    }

    /// The canvas-native painter had no Windows arm: every canvas on Windows
    /// mounted the "unsupported" placeholder, so charts drew nothing. The
    /// basics: a fill lands where its transform puts it, scaled by the DPI.
    #[test]
    fn regression_scene_fill_and_transform_paint_at_device_scale() {
        skip_under_wine!();
        let mut h = Harness::new(40, 40, 2.0);
        let ops = vec![
            DrawOp::Transform(Transform::translate(5.0, 0.0)),
            fill(Path::rect(0.0, 0.0, 5.0, 5.0), rgba(255, 0, 0, 255)),
        ];
        let f = h.render(&ops, &[]);
        assert_px(&f, 12, 4, RED);
        assert_px(&f, 8, 4, CLEAR);
        assert_px(&f, 21, 4, CLEAR);
    }

    #[test]
    fn even_odd_fill_leaves_the_hole() {
        skip_under_wine!();
        let mut h = Harness::new(30, 30, 1.0);
        let ring = Path::rect(0.0, 0.0, 30.0, 30.0)
            .move_to(10.0, 10.0)
            .line_to(20.0, 10.0)
            .line_to(20.0, 20.0)
            .line_to(10.0, 20.0)
            .close();
        let ops = vec![DrawOp::Fill { path: ring, paint: Paint::solid(rgba(0, 0, 255, 255)), fill_rule: FillRule::EvenOdd }];
        let f = h.render(&ops, &[]);
        assert_px(&f, 3, 3, BLUE);
        assert_px(&f, 15, 15, CLEAR);
    }

    #[test]
    fn linear_gradient_runs_between_its_points_and_pads() {
        skip_under_wine!();
        let mut h = Harness::new(40, 4, 1.0);
        let g = LinearGradient {
            x0: 10.0,
            y0: 0.0,
            x1: 30.0,
            y1: 0.0,
            stops: vec![
                GradientStop { offset: 0.0, color: rgba(255, 0, 0, 255) },
                GradientStop { offset: 1.0, color: rgba(0, 0, 255, 255) },
            ],
        };
        let ops = vec![DrawOp::Fill {
            path: Path::rect(0.0, 0.0, 40.0, 4.0),
            paint: Paint { kind: PaintKind::Linear(g), blend: BlendMode::Normal },
            fill_rule: FillRule::NonZero,
        }];
        let f = h.render(&ops, &[]);
        assert_px(&f, 2, 2, RED);
        assert_px(&f, 38, 2, BLUE);
        let mid = f.at(20, 2);
        assert!(mid[0] > 60 && mid[2] > 60, "the middle is a mix: {mid:?}");
    }

    #[test]
    fn dashed_stroke_has_gaps() {
        skip_under_wine!();
        let mut h = Harness::new(40, 10, 1.0);
        let mut stroke = Stroke::width(4.0);
        stroke.dash = vec![10.0];
        let ops = vec![DrawOp::Stroke {
            path: Path::new().move_to(0.0, 5.0).line_to(40.0, 5.0),
            paint: Paint::solid(rgba(255, 0, 0, 255)),
            stroke,
        }];
        let f = h.render(&ops, &[]);
        assert_px(&f, 5, 5, RED);
        assert_px(&f, 15, 5, CLEAR);
        assert_px(&f, 25, 5, RED);
    }

    /// A rectangle clip (the fast path) and a circle clip (a layer, drawn
    /// isolated) both cut the fill, and `Restore` lifts them.
    #[test]
    fn rect_and_path_clips_cut_until_restore() {
        skip_under_wine!();
        let mut h = Harness::new(40, 20, 1.0);
        let ops = vec![
            DrawOp::Save,
            DrawOp::Clip { path: Path::rect(0.0, 0.0, 10.0, 20.0), fill_rule: FillRule::NonZero },
            fill(Path::rect(0.0, 0.0, 20.0, 20.0), rgba(255, 0, 0, 255)),
            DrawOp::Restore,
            DrawOp::Save,
            DrawOp::Clip { path: Path::circle(30.0, 10.0, 5.0), fill_rule: FillRule::NonZero },
            fill(Path::rect(20.0, 0.0, 20.0, 20.0), rgba(0, 0, 255, 255)),
            DrawOp::Restore,
            fill(Path::rect(18.0, 0.0, 2.0, 2.0), rgba(255, 0, 0, 255)),
        ];
        let f = h.render(&ops, &[]);
        assert_px(&f, 5, 10, RED);
        assert_px(&f, 15, 10, CLEAR);
        assert_px(&f, 30, 10, BLUE);
        assert_px(&f, 22, 2, CLEAR);
        assert_px(&f, 19, 1, RED);
    }

    /// The whiteboard's model: a persistent layer accumulates strokes across
    /// frames, and an eraser (`DestinationOut`) inside it removes baked pixels
    /// for good. No CPU renderer had a test for the eraser.
    #[test]
    fn persistent_layer_accumulates_and_the_eraser_is_permanent() {
        skip_under_wine!();
        let mut h = Harness::new(30, 10, 1.0);
        let layer = |ops: Vec<DrawOp>| DrawOp::Layer { id: 7, clear: false, ops, alpha: 1.0, blend: BlendMode::Normal };
        h.render(&[layer(vec![fill(Path::rect(0.0, 0.0, 10.0, 10.0), rgba(255, 0, 0, 255))])], &[]);
        let f = h.render(&[layer(vec![fill(Path::rect(10.0, 0.0, 20.0, 10.0), rgba(255, 0, 0, 255))])], &[]);
        assert_px(&f, 5, 5, RED);
        assert_px(&f, 25, 5, RED);
        let eraser = DrawOp::Fill { path: Path::rect(0.0, 0.0, 15.0, 10.0), paint: Paint::eraser(), fill_rule: FillRule::NonZero };
        h.render(&[layer(vec![eraser])], &[]);
        let f = h.render(&[layer(Vec::new())], &[]);
        assert_px(&f, 5, 5, CLEAR);
        assert_px(&f, 25, 5, RED);
    }

    /// A cached layer bakes once, keeps showing with no ops, and moves with
    /// its composite transform (in logical units, at any scale).
    #[test]
    fn cached_layer_bakes_once_and_moves_with_its_transform() {
        skip_under_wine!();
        let mut h = Harness::new(60, 20, 2.0);
        let cached = |dirty: bool, dx: f32, ops: Vec<DrawOp>| DrawOp::LayerCached {
            id: 1,
            dirty,
            transform: Transform::translate(dx, 0.0),
            ops,
            alpha: 1.0,
            blend: BlendMode::Normal,
        };
        h.render(&[cached(true, 0.0, vec![fill(Path::rect(0.0, 0.0, 5.0, 5.0), rgba(255, 0, 0, 255))])], &[]);
        let f = h.render(&[cached(false, 10.0, Vec::new())], &[]);
        assert_px(&f, 25, 5, RED);
        assert_px(&f, 5, 5, CLEAR);
    }

    /// Image pixels are straight alpha: half-transparent red over nothing
    /// reads back as premultiplied (128, 0, 0, 128).
    #[test]
    fn image_blits_straight_alpha_pixels_scaled_to_dst() {
        skip_under_wine!();
        let mut h = Harness::new(20, 20, 1.0);
        let img = Arc::new(ImageSource::from_rgba8(1, 1, 1, vec![255, 0, 0, 128]));
        let ops = vec![DrawOp::Image { image: img, dst: Rect { x: 5.0, y: 5.0, w: 10.0, h: 10.0 }, alpha: 1.0, blend: BlendMode::Normal }];
        let f = h.render(&ops, &[]);
        assert_px(&f, 10, 10, [128, 0, 0, 128]);
        assert_px(&f, 2, 2, CLEAR);
    }

    /// The separable modes blend with what is already drawn: multiply of
    /// yellow over red-ish gives red; over nothing it is the source.
    #[test]
    fn multiply_blends_with_the_backdrop() {
        skip_under_wine!();
        let mut h = Harness::new(20, 10, 1.0);
        let ops = vec![
            fill(Path::rect(0.0, 0.0, 10.0, 10.0), rgba(255, 0, 255, 255)),
            DrawOp::Fill {
                path: Path::rect(0.0, 0.0, 20.0, 10.0),
                paint: Paint { kind: PaintKind::Solid(rgba(255, 255, 0, 255)), blend: BlendMode::Multiply },
                fill_rule: FillRule::NonZero,
            },
        ];
        let f = h.render(&ops, &[]);
        assert_px(&f, 5, 5, RED);
        assert_px(&f, 15, 5, [255, 255, 0, 255]);
    }

    /// A luminance mask: white keeps the content, black removes it.
    #[test]
    fn luminance_mask_group_keeps_content_under_white() {
        skip_under_wine!();
        let mut h = Harness::new(20, 10, 1.0);
        let ops = vec![DrawOp::MaskGroup {
            content: vec![fill(Path::rect(0.0, 0.0, 20.0, 10.0), rgba(0, 0, 255, 255))],
            mask: vec![
                fill(Path::rect(0.0, 0.0, 10.0, 10.0), rgba(255, 255, 255, 255)),
                fill(Path::rect(10.0, 0.0, 10.0, 10.0), rgba(0, 0, 0, 255)),
            ],
            luminance: true,
            alpha: 1.0,
            blend: BlendMode::Normal,
        }];
        let f = h.render(&ops, &[]);
        assert_px(&f, 5, 5, BLUE);
        assert_px(&f, 15, 5, CLEAR);
    }

    #[test]
    fn shapes_batch_matches_its_fills() {
        skip_under_wine!();
        let mut h = Harness::new(20, 10, 1.0);
        let ops = vec![DrawOp::Shapes {
            shapes: vec![ShapeInstance::rect(0.0, 0.0, 10.0, 10.0, rgba(255, 0, 0, 255))],
            blend: BlendMode::Normal,
        }];
        let f = h.render(&ops, &[]);
        assert_px(&f, 5, 5, RED);
        assert_px(&f, 15, 5, CLEAR);
    }

    /// A texture layer composites straight-alpha frames correctly, ignoring
    /// the author's transform, and its rounded corner is cut.
    #[test]
    fn texture_layer_composites_straight_alpha_with_rounded_corners() {
        skip_under_wine!();
        let mut h = Harness::new(20, 20, 1.0);
        let frame = Arc::new(ImageSource::from_rgba8(2, 1, 1, vec![255, 0, 0, 128]));
        let layer = TextureLayer::image(Rc::new(move || Some(frame.clone())), Rc::new(|| (0.0, 0.0, 20.0, 20.0)))
            .corner_radius(8.0);
        // The author's translation must not move the texture.
        let ops = vec![
            fill(Path::rect(0.0, 0.0, 20.0, 20.0), rgba(0, 0, 0, 255)),
            DrawOp::Transform(Transform::translate(50.0, 50.0)),
            DrawOp::Texture { index: 0 },
        ];
        let f = h.render(&ops, &[layer]);
        assert_px(&f, 10, 10, [128, 0, 0, 255]);
        assert_px(&f, 0, 0, [0, 0, 0, 255]);
    }

    thread_local! {
        static QUEUED: RefCell<Vec<Box<dyn FnOnce()>>> = RefCell::new(Vec::new());
    }

    fn queue_frame(f: Box<dyn FnOnce()>) -> Option<runtime_shared::scheduling::ScheduledTask> {
        QUEUED.with(|q| q.borrow_mut().push(f));
        None
    }

    fn run_frames() {
        let frames: Vec<_> = QUEUED.with(|q| q.borrow_mut().drain(..).collect());
        for f in frames {
            f();
        }
    }

    /// A chart's data change installs its END state first and the animation
    /// start one flush later; drawing each scene as it arrived flashed the
    /// end state before every animation. Scenes are drawn once per frame,
    /// and that frame draws the latest one.
    #[test]
    fn regression_scenes_are_drawn_once_per_frame_not_on_arrival() {
        let view = CanvasView::new(SizeReporter::new(), queue_frame);
        let scene = |n: usize| Scene::from_ops(vec![DrawOp::Save; n]);
        view.borrow_mut().install(scene(1), Vec::new());
        CanvasView::request_frame(&view);
        view.borrow_mut().install(scene(2), Vec::new());
        CanvasView::request_frame(&view);
        assert_eq!(view.borrow().frames, 0, "nothing is drawn when a scene arrives");
        run_frames();
        assert_eq!(view.borrow().frames, 1, "two scenes in a frame are one draw");
        assert_eq!(view.borrow().scene.ops().len(), 2, "of the latest scene");
        CanvasView::request_frame(&view);
        run_frames();
        assert_eq!(view.borrow().frames, 1, "nothing new, no frame");
    }

    #[test]
    fn rect_detection_needs_an_axis_aligned_rectangle() {
        let r = Path::rect(1.0, 2.0, 3.0, 4.0);
        assert_eq!(axis_aligned_rect(&r, &Aff::scale(2.0)), Some((2.0, 4.0, 8.0, 12.0)));
        let rotated = Aff { a: 0.0, b: 1.0, c: -1.0, d: 0.0, e: 0.0, f: 0.0 };
        assert_eq!(axis_aligned_rect(&r, &rotated), None);
        assert_eq!(axis_aligned_rect(&Path::circle(5.0, 5.0, 2.0), &Aff::IDENTITY), None);
    }

    #[test]
    fn dash_pattern_is_in_stroke_widths_and_odd_lists_repeat() {
        assert_eq!(dash_pattern(&[10.0], 2.0), Some(vec![5.0, 5.0]));
        assert_eq!(dash_pattern(&[4.0, 2.0], 2.0), Some(vec![2.0, 1.0]));
        assert_eq!(dash_pattern(&[], 2.0), None);
        assert_eq!(dash_pattern(&[0.0, 0.0], 2.0), None);
    }

    #[test]
    fn transforms_compose_like_canvas2d() {
        // translate then scale: (1,1) → scale → (2,2) → translate → (12, 2).
        let ctm = Aff::of(&Transform::translate(10.0, 0.0));
        let ctm = Aff::of(&Transform::scale(2.0, 2.0)).then(&ctm);
        assert_eq!(ctm.apply(1.0, 1.0), (12.0, 2.0));
    }
}
