//! The Direct2D renderer: the painted scene drawn with Direct2D +
//! DirectWrite into a DirectComposition-hosted swapchain.
//!
//! ## Why
//!
//! The GDI+ painter (`scene.rs`) rasterizes every node on the CPU on every
//! frame. Measured on the idea-ui docs app (2026-10-15, Wine 11.18 at 2×):
//! ~140 ms per scroll frame, ~90 ms of it `GdipDrawString` and ~45 ms view
//! fills, with every node drawn whether visible or not. Direct2D draws the
//! same primitives on the GPU and DirectWrite is the platform's text stack
//! (the one WinUI and browsers use), so this renderer keeps the backend's
//! scene model and replaces only how it reaches pixels.
//!
//! ## Shape
//!
//! - **Same scene, same rules.** The walk mirrors `scene::paint_node`: frame
//!   offset, then the `scene::transform_ops` chain (composed with
//!   `painted_geom::Affine`, the same maths the introspection reader uses),
//!   content inset, own visual, `overflow`/scroll clip, scroll offset,
//!   children in `scene::z_sorted` order. Group opacity multiplies into each
//!   drawn color, as the GDI painter does (images use Direct2D's per-draw
//!   opacity, closing the GDI path's documented image gap).
//! - **Culling.** A node whose device bounds miss the current clip is not
//!   drawn; a clipping node that misses it skips its whole subtree.
//! - **Text** is measured by `dwrite_text` (see there for why measurement
//!   must move with the renderer) and drawn from cached `IDWriteTextLayout`s,
//!   one per wrapped line.
//! - **Presentation.** One flip-model swapchain created for composition, set
//!   as the content of the BOTTOM visual of the backend's composition tree
//!   (`dcomp.rs`): graphics visuals stack above it and the window's native
//!   controls above those, exactly as with the GDI painter. This is the one-surface
//!   strategy: it needs only composition swapchains, which work on every
//!   DirectComposition implementation, including Wine-Staging's (which lacks
//!   `IDCompositionDevice::CreateSurface`).
//!
//! Selected at startup by `IDEALYST_WINDOWS_RENDERER=d2d` (see
//! [`crate::RendererKind`]); the GDI painter stays the default until this
//! path is at parity.

use std::collections::HashMap;

use windows::core::{Interface, Result};
use windows::Win32::Foundation::HMODULE;
use windows_numerics::{Matrix3x2, Vector2};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_BEGIN_HOLLOW,
    D2D1_FIGURE_END_CLOSED, D2D1_FIGURE_END_OPEN, D2D1_FILL_MODE_ALTERNATE, D2D1_FILL_MODE_WINDING,
    D2D1_GRADIENT_STOP, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_F, D2D_SIZE_U,
    D2D1_BEZIER_SEGMENT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1Brush, ID2D1DeviceContext, ID2D1Factory1, ID2D1Geometry,
    ID2D1PathGeometry, ID2D1RenderTarget, ID2D1SolidColorBrush, ID2D1StrokeStyle,
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ARC_SEGMENT, D2D1_ARC_SIZE_SMALL,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_BITMAP_OPTIONS, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_CPU_READ, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_CAP_STYLE_FLAT, D2D1_CAP_STYLE_ROUND, D2D1_DEVICE_CONTEXT_OPTIONS_NONE,
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_EXTEND_MODE_CLAMP,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_GAMMA_2_2, D2D1_LAYER_OPTIONS1_NONE,
    D2D1_LAYER_PARAMETERS1, D2D1_LINE_JOIN_ROUND, D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES,
    D2D1_MAP_OPTIONS_READ, D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES, D2D1_ROUNDED_RECT,
    D2D1_STROKE_STYLE_PROPERTIES, D2D1_SWEEP_DIRECTION_CLOCKWISE,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, ID2D1GeometrySink,
};
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, D3D11_CREATE_DEVICE_BGRA_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::IDCompositionVisual;
use windows::Win32::Graphics::DirectWrite::IDWriteTextLayout;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_IGNORE, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_PRESENT,
    DXGI_SCALING_STRETCH, DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG,
    DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL, DXGI_USAGE_RENDER_TARGET_OUTPUT,
};

use crate::border_pattern::{self, BorderMark};
use crate::font::FontKey;
use crate::icon::{self, IconPaint, PathEmitter};
use crate::painted_geom::{self, Affine};
use crate::scene;
use crate::{
    uniform_border, BorderSide, GradKind, GradientPaint, NodeKind, ViewVisual,
    WindowsBackend,
};
use runtime_shared::color::Rgba;
use runtime_shared::TextAlign;

/// The device-independent half: D3D11 device, Direct2D context and the
/// caches. Drawing targets (window swapchain or an offscreen bitmap) are
/// set per frame, so tests and screenshots render through the same walk.
pub(crate) struct D2dRenderer {
    d3d: ID3D11Device,
    factory: ID2D1Factory1,
    ctx: ID2D1DeviceContext,
    brush: ID2D1SolidColorBrush,
    /// Round caps + joins (outlined icons).
    round_stroke: ID2D1StrokeStyle,
    /// Flat caps, round joins (dashed borders — see `border_pattern`).
    dash_stroke: ID2D1StrokeStyle,
    window: Option<WindowTarget>,
    text_cache: HashMap<u64, TextEntry>,
    icon_cache: HashMap<u64, IconEntry>,
    image_cache: HashMap<u64, (usize, ID2D1Bitmap1)>,
    /// The frame's layer plan and the layer being drawn: a walk draws only
    /// the nodes `plan` puts in `layer` (`crate::layers`).
    plan: crate::layers::LayerPlan,
    layer: usize,
}

/// The scene's swapchain and the visual showing it: the bottom child of the
/// backend's composition tree ([`crate::dcomp::CompositionTree`]), under the
/// graphics visuals, which are under the native controls.
struct WindowTarget {
    _visual: IDCompositionVisual,
    swap: IDXGISwapChain1,
    size: (u32, u32),
    /// The back buffer as a Direct2D target. Flip-model buffer 0 always
    /// names the current back buffer, so one wrapper serves every frame
    /// until a resize.
    bitmap: Option<ID2D1Bitmap1>,
}

/// Cached layouts for one text node, valid while `lines` and `key` match.
struct TextEntry {
    key: FontKey,
    lines: Vec<Vec<u16>>,
    layouts: Vec<IDWriteTextLayout>,
}

/// Cached view-box geometry for one icon node.
struct IconEntry {
    paths: Vec<String>,
    even_odd: bool,
    geometry: ID2D1PathGeometry,
}

fn color(rgba: [f32; 4], alpha: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: rgba[0], g: rgba[1], b: rgba[2], a: (rgba[3] * alpha).clamp(0.0, 1.0) }
}

fn color8(c: Rgba, alpha: f32) -> D2D1_COLOR_F {
    color([c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, c.a as f32 / 255.0], alpha)
}

fn matrix(m: &Affine) -> Matrix3x2 {
    Matrix3x2 { M11: m.a, M12: m.b, M21: m.c, M22: m.d, M31: m.e, M32: m.f }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> D2D_RECT_F {
    D2D_RECT_F { left: x, top: y, right: x + w, bottom: y + h }
}

fn pt(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn bitmap_props(opts: D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        // 96 = one DIP per pixel: the walk's root transform applies the
        // DPI scale itself, exactly as the GDI painter's does.
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: opts,
        colorContext: std::mem::ManuallyDrop::new(None),
    }
}

/// Intersection of two device rects `(x0, y0, x1, y1)`.
fn intersect(a: (f32, f32, f32, f32), b: (f32, f32, f32, f32)) -> (f32, f32, f32, f32) {
    (a.0.max(b.0), a.1.max(b.1), a.2.min(b.2), a.3.min(b.3))
}

fn misses(bounds: (f32, f32, f32, f32), clip: (f32, f32, f32, f32)) -> bool {
    let (x, y, w, h) = bounds;
    x >= clip.2 || y >= clip.3 || x + w <= clip.0 || y + h <= clip.1
}

impl D2dRenderer {
    pub(crate) fn new() -> Result<Self> {
        unsafe {
            let mut d3d: Option<ID3D11Device> = None;
            D3D11CreateDevice(
                None,
                D3D_DRIVER_TYPE_HARDWARE,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                None,
                D3D11_SDK_VERSION,
                Some(&mut d3d),
                None,
                None,
            )?;
            let d3d = d3d.ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
            let dxgi: IDXGIDevice = d3d.cast()?;
            let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let device = factory.CreateDevice(&dxgi)?;
            let ctx = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            // The GDI painter deliberately avoids ClearType (it fringes over
            // translucent fills); grayscale keeps the two renderers alike.
            ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let brush = ctx.CreateSolidColorBrush(&color([0.0, 0.0, 0.0, 1.0], 1.0), None)?;
            let base: windows::Win32::Graphics::Direct2D::ID2D1Factory = factory.cast()?;
            let round_stroke = base.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    ..Default::default()
                },
                None,
            )?;
            let dash_stroke = base.CreateStrokeStyle(
                &D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_FLAT,
                    endCap: D2D1_CAP_STYLE_FLAT,
                    dashCap: D2D1_CAP_STYLE_FLAT,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    ..Default::default()
                },
                None,
            )?;
            Ok(D2dRenderer {
                d3d,
                factory,
                ctx,
                brush,
                round_stroke,
                dash_stroke,
                window: None,
                text_cache: HashMap::new(),
                icon_cache: HashMap::new(),
                image_cache: HashMap::new(),
                plan: crate::layers::LayerPlan::default(),
                layer: 0,
            })
        }
    }

    /// Draw the scene into the window at `w × h` device px and present.
    pub(crate) fn render_window(&mut self, b: &WindowsBackend, w: i32, h: i32) -> Result<()> {
        if w <= 0 || h <= 0 {
            return Ok(());
        }
        let size = (w as u32, h as u32);
        unsafe {
            if self.window.is_none() {
                let tree = b.comp.as_ref().ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
                self.window = Some(self.create_window_target(tree, size)?);
            }
            let win = self.window.as_mut().expect("just created");
            if win.size != size {
                // Every reference to the old buffers must go first.
                self.ctx.SetTarget(None);
                win.bitmap = None;
                win.swap.ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
                win.size = size;
            }
            if win.bitmap.is_none() {
                let surface: IDXGISurface = win.swap.GetBuffer(0)?;
                win.bitmap = Some(self.ctx.CreateBitmapFromDxgiSurface(
                    &surface,
                    Some(&bitmap_props(D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW)),
                )?);
            }
            let target = win.bitmap.clone().expect("just created");
            self.plan = b.layer_plan();
            self.layer = 0;
            self.draw(b, &target, size, (0.0, 0.0), true)?;
            // Sync interval 1: present on the next vblank. Paints are driven
            // by WM_PAINT / the 60 Hz scheduler pulse, so this paces rather
            // than queues frames.
            let win = self.window.as_ref().expect("present");
            win.swap.Present(1, DXGI_PRESENT(0)).ok()?;

            // Content over a graphics surface, in visuals above it.
            let lifted: Vec<(usize, u64, crate::layers::Rect)> =
                self.plan.lifted().filter_map(|(k, o)| Some((k, o.above, o.bounds?))).collect();
            let mut layers = Vec::with_capacity(lifted.len());
            for (k, above, bounds) in lifted {
                let bounds = crate::layers::pixel_box(bounds);
                let pixels = self.render_layer(b, k, bounds)?;
                layers.push(crate::overlay::LayerPixels { above, bounds, pixels });
            }
            self.layer = 0;
            b.present_overlays(layers);
        }
        Ok(())
    }

    /// Lifted layer `layer` as premultiplied BGRA pixels for the device
    /// box `(x, y, w, h)`, transparent where it draws nothing.
    unsafe fn render_layer(&mut self, b: &WindowsBackend, layer: usize, (x, y, w, h): (i32, i32, u32, u32)) -> Result<Vec<u8>> {
        let du = D2D_SIZE_U { width: w, height: h };
        let target = self.ctx.CreateBitmap(du, None, 0, &bitmap_props(D2D1_BITMAP_OPTIONS_TARGET))?;
        self.layer = layer;
        self.draw(b, &target, (w, h), (x as f32, y as f32), false)?;
        self.read_back(&target, (w, h))
    }

    unsafe fn create_window_target(&self, tree: &crate::dcomp::CompositionTree, size: (u32, u32)) -> Result<WindowTarget> {
        let dxgi: IDXGIDevice = self.d3d.cast()?;
        let adapter = dxgi.GetAdapter()?;
        let dxgi_factory: IDXGIFactory2 = adapter.GetParent()?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: size.0,
            Height: size.1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
            // Opaque: the scene clears to the app background every frame.
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            ..Default::default()
        };
        let swap = dxgi_factory.CreateSwapChainForComposition(&self.d3d, &desc, None)?;
        let visual = tree
            .add_scene_visual()
            .ok_or_else(|| windows::core::Error::from_hresult(windows::Win32::Foundation::E_FAIL))?;
        visual.SetContent(&swap)?;
        tree.commit();
        Ok(WindowTarget { _visual: visual, swap, size, bitmap: None })
    }

    /// Draw the scene into a fresh offscreen bitmap and read it back as
    /// top-down premultiplied BGRA rows (`w * 4` bytes each). Screenshots
    /// and the pixel tests use this — the same walk as the window path.
    #[cfg_attr(not(any(test, feature = "robot")), allow(dead_code))]
    pub(crate) fn render_offscreen(&mut self, b: &WindowsBackend, w: i32, h: i32) -> Result<Vec<u8>> {
        let size = (w.max(1) as u32, h.max(1) as u32);
        unsafe {
            let du = D2D_SIZE_U { width: size.0, height: size.1 };
            let target = self.ctx.CreateBitmap(du, None, 0, &bitmap_props(D2D1_BITMAP_OPTIONS_TARGET))?;
            // A screenshot is the whole scene in tree order: no lifting.
            self.plan = crate::layers::LayerPlan::default();
            self.layer = 0;
            self.draw(b, &target, size, (0.0, 0.0), true)?;
            self.read_back(&target, size)
        }
    }

    /// Copy a target bitmap back as top-down premultiplied BGRA rows.
    unsafe fn read_back(&self, target: &ID2D1Bitmap1, size: (u32, u32)) -> Result<Vec<u8>> {
        let du = D2D_SIZE_U { width: size.0, height: size.1 };
        {
            let cpu = self.ctx.CreateBitmap(
                du,
                None,
                0,
                &bitmap_props(D2D1_BITMAP_OPTIONS_CPU_READ | D2D1_BITMAP_OPTIONS_CANNOT_DRAW),
            )?;
            cpu.CopyFromBitmap(None, target, None)?;
            let map = cpu.Map(D2D1_MAP_OPTIONS_READ)?;
            let row = size.0 as usize * 4;
            let mut out = Vec::with_capacity(row * size.1 as usize);
            for y in 0..size.1 as usize {
                let src = std::slice::from_raw_parts(map.bits.add(y * map.pitch as usize), row);
                out.extend_from_slice(src);
            }
            cpu.Unmap()?;
            Ok(out)
        }
    }

    /// Draw the scene into `target`, the window's device space shifted by
    /// `origin` (a lifted layer's box). `opaque` clears to the app
    /// background (the scene); otherwise to transparent (a layer).
    unsafe fn draw(
        &mut self,
        b: &WindowsBackend,
        target: &ID2D1Bitmap1,
        size: (u32, u32),
        origin: (f32, f32),
        opaque: bool,
    ) -> Result<()> {
        self.prune(b);
        self.ctx.SetTarget(target);
        self.ctx.BeginDraw();
        let clear = if opaque {
            b.app_background.map(|c| color8(c, 1.0)).unwrap_or(color([1.0, 1.0, 1.0, 1.0], 1.0))
        } else {
            color([0.0, 0.0, 0.0, 0.0], 0.0)
        };
        self.ctx.Clear(Some(&clear));
        let scale = b.dpi.factor();
        let root = Affine { a: scale, d: scale, e: -origin.0, f: -origin.1, ..Affine::IDENTITY };
        let clip = (0.0, 0.0, size.0 as f32, size.1 as f32);
        if let Some(root_id) = b.root_id {
            self.node(b, root_id, &root, 1.0, clip);
        }
        let r = self.ctx.EndDraw(None, None);
        self.ctx.SetTarget(None);
        r
    }

    /// Drop cache entries for nodes that no longer exist.
    fn prune(&mut self, b: &WindowsBackend) {
        self.text_cache.retain(|id, _| b.nodes.contains_key(id));
        self.icon_cache.retain(|id, _| b.nodes.contains_key(id));
        self.image_cache.retain(|id, _| b.nodes.contains_key(id));
    }

    /// One node + subtree. `parent` maps the parent's content space to
    /// device px; `clip` is the device-space clip bounds so far.
    unsafe fn node(&mut self, b: &WindowsBackend, id: u64, parent: &Affine, alpha: f32, clip: (f32, f32, f32, f32)) {
        let Some(meta) = b.nodes.get(&id) else {
            return;
        };
        if meta.hidden {
            return;
        }
        let alpha = alpha * meta.effective_opacity();
        if alpha <= 0.003 {
            return;
        }
        let (fx, fy, w, h) = meta.frame;
        let ops = scene::transform_ops(w, h, &meta.author_transform, &meta.anim);
        let m = parent.node(fx, fy, &ops);
        let bounds = m.bounds(w, h);
        let children = b.children.get(&id).filter(|c| !c.is_empty());
        let clips = meta.clips_children();

        // Own visual, unless it can't be seen or is in another layer.
        if !misses(bounds, clip) && self.plan.layer(id) == self.layer {
            let (cx, cy, cw, ch) = painted_geom::content_box(w, h, meta.content_inset);
            let content = m.then_translate(cx, cy);
            self.ctx.SetTransform(&matrix(&content));
            match &meta.kind {
                NodeKind::View(v) => self.view(v, cw, ch, alpha),
                NodeKind::Text(_) => self.text(b, id, cw, alpha),
                NodeKind::Code(_) => self.code(b, id, alpha),
                NodeKind::Icon(p) => self.icon(id, p, cw, ch, &content, alpha),
                NodeKind::Image(p) => self.image(b, id, p, cw, ch, alpha),
                NodeKind::Control { .. } | NodeKind::External { .. } => {}
            }
        } else if clips || children.is_none() {
            // Nothing of it or (clipped) its subtree can show.
            return;
        }

        let Some(children) = children else {
            return;
        };
        let mut child_clip = clip;
        let mut layer = false;
        if clips {
            child_clip = intersect(clip, {
                let (x, y, bw, bh) = bounds;
                (x, y, x + bw, y + bh)
            });
            if child_clip.0 >= child_clip.2 || child_clip.1 >= child_clip.3 {
                return;
            }
            self.ctx.SetTransform(&matrix(&m));
            let radii = match &meta.kind {
                NodeKind::View(v) => v.radii,
                _ => [0.0; 4],
            };
            // The box clip always, and for rounded corners a geometric
            // layer inside it. The axis-aligned clip alone is exact for a
            // square box; under the layer it is redundant on Windows but
            // keeps children inside the box where layers are unavailable
            // (Wine's `PushLayer`/`PopLayer` are stubs), so the worst case
            // there is square corners, not unclipped content.
            self.ctx.PushAxisAlignedClip(&rect(0.0, 0.0, w, h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            if let Some(geometry) = self.round_rect_geometry(w, h, radii, 0.0) {
                let params = D2D1_LAYER_PARAMETERS1 {
                    contentBounds: rect(0.0, 0.0, w, h),
                    geometricMask: std::mem::ManuallyDrop::new(Some(geometry)),
                    maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
                    maskTransform: Matrix3x2::identity(),
                    opacity: 1.0,
                    opacityBrush: std::mem::ManuallyDrop::new(None),
                    layerOptions: D2D1_LAYER_OPTIONS1_NONE,
                };
                self.ctx.PushLayer(&params, None);
                std::mem::ManuallyDrop::into_inner(params.geometricMask);
                layer = true;
            }
        }
        let mut cm = m;
        if let NodeKind::View(v) = &meta.kind {
            if let Some(s) = &v.scroll {
                cm = cm.then_translate(-s.offset_x, -s.offset_y);
            }
        }
        let pairs: Vec<(u64, f32)> =
            children.iter().filter_map(|cid| b.nodes.get(cid).map(|cm| (*cid, cm.z))).collect();
        for cid in scene::z_sorted(&pairs) {
            self.node(b, cid, &cm, alpha, child_clip);
        }
        if clips {
            if layer {
                self.ctx.PopLayer();
            }
            self.ctx.PopAxisAlignedClip();
        }
    }

    fn solid(&self, c: D2D1_COLOR_F) -> ID2D1Brush {
        unsafe { self.brush.SetColor(&c) };
        self.brush.cast().expect("solid brush is a brush")
    }

    /// The node's rounded-rect shape inset by `inset`, radii clamped as
    /// `scene::build_round_rect_path` does. `None` for a square box (the
    /// caller uses the plain-rect calls).
    unsafe fn round_rect_geometry(&self, w: f32, h: f32, radii: [f32; 4], inset: f32) -> Option<ID2D1Geometry> {
        if radii.iter().all(|r| *r < 0.5) {
            return None;
        }
        let (left, top) = (inset, inset);
        let right = (w - inset).max(left);
        let bottom = (h - inset).max(top);
        let rmax = (right - left).min(bottom - top) / 2.0;
        let c = |i: usize| radii[i].clamp(0.0, rmax.max(0.0));
        let (tl, tr, br, bl) = (c(0), c(1), c(2), c(3));
        if tl == tr && tr == br && br == bl {
            let rr = D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left, top, right, bottom }, radiusX: tl, radiusY: tl };
            return self.factory.CreateRoundedRectangleGeometry(&rr).ok().and_then(|g| g.cast().ok());
        }
        let path = self.factory.CreatePathGeometry().ok()?;
        let sink = path.Open().ok()?;
        let arc = |sink: &ID2D1GeometrySink, r: f32, to: (f32, f32)| {
            if r >= 0.5 {
                sink.AddArc(&D2D1_ARC_SEGMENT {
                    point: pt(to.0, to.1),
                    size: D2D_SIZE_F { width: r, height: r },
                    rotationAngle: 0.0,
                    sweepDirection: D2D1_SWEEP_DIRECTION_CLOCKWISE,
                    arcSize: D2D1_ARC_SIZE_SMALL,
                });
            }
        };
        let tl_r = if tl >= 0.5 { tl } else { 0.0 };
        let tr_r = if tr >= 0.5 { tr } else { 0.0 };
        let br_r = if br >= 0.5 { br } else { 0.0 };
        let bl_r = if bl >= 0.5 { bl } else { 0.0 };
        sink.BeginFigure(pt(left + tl_r, top), D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(pt(right - tr_r, top));
        arc(&sink, tr_r, (right, top + tr_r));
        sink.AddLine(pt(right, bottom - br_r));
        arc(&sink, br_r, (right - br_r, bottom));
        sink.AddLine(pt(left + bl_r, bottom));
        arc(&sink, bl_r, (left, bottom - bl_r));
        sink.AddLine(pt(left, top + tl_r));
        arc(&sink, tl_r, (left + tl_r, top));
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close().ok()?;
        path.cast().ok()
    }

    unsafe fn view(&mut self, v: &ViewVisual, w: f32, h: f32, alpha: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let bg = v.effective_background().filter(|c| c[3] > 0.0);
        let gradient = v.gradient.as_ref().filter(|g| !g.stops.is_empty());
        if bg.is_some() || gradient.is_some() {
            let shape = self.round_rect_geometry(w, h, v.radii, 0.0);
            let fill = |brush: &ID2D1Brush| match &shape {
                Some(g) => self.ctx.FillGeometry(g, brush, None),
                None => self.ctx.FillRectangle(&rect(0.0, 0.0, w, h), brush),
            };
            if let Some(c) = bg {
                let brush = self.solid(color(c, alpha));
                fill(&brush);
            }
            if let Some(gp) = gradient {
                if let Some(brush) = self.gradient_brush(gp, w, h, alpha) {
                    fill(&brush);
                }
            }
        }
        self.border(v, w, h, alpha);
    }

    /// Gradient brush in the node's local space. Stops follow the GDI
    /// path's `blend_arrays` (clamped, ascending); Direct2D clamps outside
    /// the first/last stop itself.
    unsafe fn gradient_brush(&self, gp: &GradientPaint, w: f32, h: f32, alpha: f32) -> Option<ID2D1Brush> {
        let mut stops: Vec<D2D1_GRADIENT_STOP> = gp
            .stops
            .iter()
            .map(|(o, c)| D2D1_GRADIENT_STOP { position: o.clamp(0.0, 1.0), color: color(*c, alpha) })
            .collect();
        stops.sort_by(|a, b| a.position.partial_cmp(&b.position).unwrap_or(std::cmp::Ordering::Equal));
        // The Direct2D 1.0 overload: sRGB (gamma 2.2) interpolation, and the
        // one every implementation has (Wine's 1.1 overload is a stub).
        let rt: ID2D1RenderTarget = self.ctx.cast().ok()?;
        let collection = rt.CreateGradientStopCollection(&stops, D2D1_GAMMA_2_2, D2D1_EXTEND_MODE_CLAMP).ok()?;
        match gp.kind {
            GradKind::Linear { angle_deg } => {
                let (s, e) = crate::linear_points(angle_deg, w, h);
                let brush = rt
                    .CreateLinearGradientBrush(
                        &D2D1_LINEAR_GRADIENT_BRUSH_PROPERTIES { startPoint: pt(s.0, s.1), endPoint: pt(e.0, e.1) },
                        None,
                        &collection,
                    )
                    .ok()?;
                brush.cast().ok()
            }
            GradKind::Radial { center, radius, farthest } => {
                let r = crate::radial_radius(center, radius, farthest, w, h).max(1.0);
                let brush = rt
                    .CreateRadialGradientBrush(
                        &D2D1_RADIAL_GRADIENT_BRUSH_PROPERTIES {
                            center: pt(center.0 * w, center.1 * h),
                            gradientOriginOffset: pt(0.0, 0.0),
                            radiusX: r,
                            radiusY: r,
                        },
                        None,
                        &collection,
                    )
                    .ok()?;
                brush.cast().ok()
            }
        }
    }

    /// Borders: patterned marks, one stroked rounded path when uniform,
    /// else per-side bars — the same split as `scene::paint_border`.
    unsafe fn border(&mut self, v: &ViewVisual, w: f32, h: f32, alpha: f32) {
        if let Some(marks) = border_pattern::plan(&v.borders, v.radii, v.border_style, w, h) {
            for mark in marks {
                match mark {
                    BorderMark::Dash { points, width, color: c } => {
                        if points.len() < 2 {
                            continue;
                        }
                        let Ok(path) = self.factory.CreatePathGeometry() else { continue };
                        let Ok(sink) = path.Open() else { continue };
                        sink.BeginFigure(pt(points[0].0, points[0].1), D2D1_FIGURE_BEGIN_HOLLOW);
                        let rest: Vec<Vector2> = points[1..].iter().map(|(x, y)| pt(*x, *y)).collect();
                        sink.AddLines(&rest);
                        sink.EndFigure(D2D1_FIGURE_END_OPEN);
                        if sink.Close().is_ok() {
                            let brush = self.solid(color8(c, alpha));
                            self.ctx.DrawGeometry(&path, &brush, width, &self.dash_stroke);
                        }
                    }
                    BorderMark::Dot { cx, cy, radius, color: c } => {
                        let brush = self.solid(color8(c, alpha));
                        self.ctx.FillEllipse(
                            &D2D1_ELLIPSE { point: pt(cx, cy), radiusX: radius, radiusY: radius },
                            &brush,
                        );
                    }
                }
            }
            return;
        }
        match uniform_border(&v.borders) {
            Some((width, c)) => {
                if width <= 0.0 || c.a == 0 {
                    return;
                }
                let brush = self.solid(color8(c, alpha));
                let half = width / 2.0;
                match self.round_rect_geometry(w, h, v.radii, half) {
                    Some(g) => self.ctx.DrawGeometry(&g, &brush, width, None),
                    None => self.ctx.DrawRectangle(
                        &D2D_RECT_F { left: half, top: half, right: (w - half).max(half), bottom: (h - half).max(half) },
                        &brush,
                        width,
                        None,
                    ),
                }
            }
            None if v.borders.iter().any(|s| s.width > 0.5) => self.side_borders(&v.borders, w, h, alpha),
            None => {}
        }
    }

    unsafe fn side_borders(&mut self, sides: &[BorderSide; 4], w: f32, h: f32, alpha: f32) {
        let fallback = sides.iter().find_map(|s| s.color);
        for (idx, side) in sides.iter().enumerate() {
            if side.width <= 0.5 {
                continue;
            }
            let Some(c) = side.color.or(fallback) else { continue };
            if c.a == 0 {
                continue;
            }
            let bw = side.width;
            let r = match idx {
                0 => rect(0.0, 0.0, w, bw),
                1 => rect(w - bw, 0.0, bw, h),
                2 => rect(0.0, h - bw, w, bw),
                _ => rect(0.0, 0.0, bw, h),
            };
            let brush = self.solid(color8(c, alpha));
            self.ctx.FillRectangle(&r, &brush);
        }
    }

    /// Text: one cached layout per wrapped line, placed as the GDI painter
    /// places its `GdipDrawString` calls (alignment + CSS half-leading).
    unsafe fn text(&mut self, b: &WindowsBackend, id: u64, w: f32, alpha: f32) {
        let Some(meta) = b.nodes.get(&id) else { return };
        let NodeKind::Text(t) = &meta.kind else { return };
        if t.content.is_empty() {
            return;
        }
        let Some(dw) = b.dwrite.as_ref() else { return };
        let key = t.font_key.clone().unwrap_or_else(|| b.default_font_key.clone());
        let c = t.anim_color.map(|c| color(c, alpha)).unwrap_or_else(|| color8(t.color, alpha));

        // (line text, x, y) in content space.
        let mut placed: Vec<(Vec<u16>, f32, f32)> = Vec::new();
        match (&t.lines, &t.plan) {
            (Some(cache), Some(plan)) => {
                let advance = t.line_height.unwrap_or(plan.font_height).max(1.0);
                let lead = ((advance - plan.font_height) / 2.0).max(0.0);
                for (i, line) in cache.lines.iter().enumerate() {
                    if line.text16.is_empty() {
                        continue;
                    }
                    let x = match t.align {
                        TextAlign::Center => ((w - line.width) / 2.0).max(0.0),
                        TextAlign::Right => (w - line.width).max(0.0),
                        TextAlign::Left | TextAlign::Justify => 0.0,
                    };
                    placed.push((line.text16.clone(), x, lead + i as f32 * advance));
                }
            }
            _ => placed.push((t.content.encode_utf16().collect(), 0.0, 0.0)),
        }
        let lines: Vec<Vec<u16>> = placed.iter().map(|(l, _, _)| l.clone()).collect();
        let fresh = match self.text_cache.get(&id) {
            Some(e) => e.key != key || e.lines != lines,
            None => true,
        };
        if fresh {
            let layouts: Vec<IDWriteTextLayout> = lines.iter().filter_map(|l| dw.layout(l, &key)).collect();
            if layouts.len() != lines.len() {
                return;
            }
            self.text_cache.insert(id, TextEntry { key, lines, layouts });
        }
        let brush = self.solid(c);
        let entry = self.text_cache.get(&id).expect("cached above");
        for ((_, x, y), layout) in placed.iter().zip(&entry.layouts) {
            self.ctx.DrawTextLayout(pt(*x, *y), layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
        }
    }

    /// Colored code runs: one layout per run at its measured x.
    unsafe fn code(&mut self, b: &WindowsBackend, id: u64, alpha: f32) {
        let Some(meta) = b.nodes.get(&id) else { return };
        let NodeKind::Code(c) = &meta.kind else { return };
        let Some(dw) = b.dwrite.as_ref() else { return };
        let lines: Vec<Vec<u16>> =
            c.lines.iter().flat_map(|l| l.iter().map(|r| r.text16.clone())).collect();
        let fresh = match self.text_cache.get(&id) {
            Some(e) => e.key != c.font_key || e.lines != lines,
            None => true,
        };
        if fresh {
            let layouts: Vec<IDWriteTextLayout> =
                lines.iter().filter_map(|l| dw.layout(l, &c.font_key)).collect();
            if layouts.len() != lines.len() {
                return;
            }
            self.text_cache.insert(id, TextEntry { key: c.font_key.clone(), lines, layouts });
        }
        let entry = self.text_cache.get(&id).expect("cached above");
        let mut layouts = entry.layouts.iter();
        for (i, line) in c.lines.iter().enumerate() {
            let y = i as f32 * c.line_height;
            let mut x = 0.0_f32;
            for run in line {
                let Some(layout) = layouts.next() else { return };
                if !run.text16.is_empty() {
                    unsafe { self.brush.SetColor(&color8(run.color, alpha)) };
                    let brush: ID2D1Brush = self.brush.cast().expect("brush");
                    self.ctx.DrawTextLayout(pt(x, y), layout, &brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
                }
                x += run.width;
            }
        }
    }

    /// Icons: the view-box geometry is built once per glyph and drawn under
    /// the fit transform, so the stroke width scales with it exactly as the
    /// GDI painter's `BASE_STROKE_WIDTH × scale` pen does.
    unsafe fn icon(&mut self, id: u64, p: &IconPaint, w: f32, h: f32, content: &Affine, alpha: f32) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let fresh = match self.icon_cache.get(&id) {
            Some(e) => e.paths != p.paths || e.even_odd != p.even_odd,
            None => true,
        };
        if fresh {
            let Some(geometry) = self.icon_geometry(p) else { return };
            self.icon_cache.insert(id, IconEntry { paths: p.paths.clone(), even_odd: p.even_odd, geometry });
        }
        let (vw, vh) = (p.view_box.0.max(1) as f32, p.view_box.1.max(1) as f32);
        let scale = (w / vw).min(h / vh);
        let ox = (w - vw * scale) / 2.0;
        let oy = (h - vh * scale) / 2.0;
        let fit = content.then_translate(ox, oy).then_op(painted_geom::XOp::Scale(scale, scale));
        self.ctx.SetTransform(&matrix(&fit));
        let brush = self.solid(color8(p.color, alpha));
        let geometry = &self.icon_cache.get(&id).expect("cached above").geometry;
        if p.filled {
            self.ctx.FillGeometry(geometry, &brush, None);
        } else {
            self.ctx.DrawGeometry(geometry, &brush, icon::BASE_STROKE_WIDTH, &self.round_stroke);
        }
    }

    unsafe fn icon_geometry(&self, p: &IconPaint) -> Option<ID2D1PathGeometry> {
        let path: ID2D1PathGeometry = self.factory.CreatePathGeometry().ok()?.cast().ok()?;
        let sink = path.Open().ok()?;
        sink.SetFillMode(if p.even_odd { D2D1_FILL_MODE_ALTERNATE } else { D2D1_FILL_MODE_WINDING });
        let mut emitter = SinkEmitter { sink: &sink, open: false, fill: p.filled, start: (0.0, 0.0) };
        for d in &p.paths {
            icon::parse_svg_path(d, 1.0, 1.0, 0.0, 0.0, &mut emitter);
        }
        emitter.finish();
        sink.Close().ok()?;
        Some(path)
    }

    /// Images: the decoded GDI+ bitmap uploaded once per source and drawn
    /// with Direct2D's per-draw opacity, clipped to the box (`Cover`).
    unsafe fn image(&mut self, b: &WindowsBackend, id: u64, p: &crate::image::ImagePaint, w: f32, h: f32, alpha: f32) {
        let ctx = self.ctx.clone();
        let cache = &mut self.image_cache;
        crate::image::with_drawable(p, w, h, b.dpi.svg_screen_scale(), |image, (dx, dy, dw, dh)| {
            let key = image.identity();
            let stale = cache.get(&id).is_none_or(|(k, _)| *k != key);
            if stale {
                let Some((iw, ih, pixels)) = image.pargb_pixels() else { return };
                let Ok(bmp) = ctx.CreateBitmap(
                    D2D_SIZE_U { width: iw, height: ih },
                    Some(pixels.as_ptr() as *const _),
                    iw * 4,
                    &bitmap_props(D2D1_BITMAP_OPTIONS(0)),
                ) else {
                    return;
                };
                cache.insert(id, (key, bmp));
            }
            let Some((_, bmp)) = cache.get(&id) else { return };
            ctx.PushAxisAlignedClip(&rect(0.0, 0.0, w, h), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            ctx.DrawBitmap(
                bmp,
                Some(&rect(dx as f32, dy as f32, dw.max(1) as f32, dh.max(1) as f32)),
                alpha.clamp(0.0, 1.0),
                D2D1_INTERPOLATION_MODE_LINEAR,
                None,
                None,
            );
            ctx.PopAxisAlignedClip();
        });
    }
}

/// SVG path commands → a Direct2D geometry sink. Direct2D needs explicit
/// figure begin/end: a `move_to` while a figure is open ends it open, and
/// `close` ends it closed. A drawing command after a `close` (SVG lets a
/// subpath continue after `Z`) starts a new figure at the closed one's start
/// point — SVG's current point after `Z` — as the GDI+ emitter's implicit
/// figures do.
struct SinkEmitter<'a> {
    sink: &'a ID2D1GeometrySink,
    open: bool,
    fill: bool,
    start: (f32, f32),
}

impl SinkEmitter<'_> {
    fn finish(&mut self) {
        if self.open {
            unsafe { self.sink.EndFigure(D2D1_FIGURE_END_OPEN) };
            self.open = false;
        }
    }
    fn ensure_open(&mut self) {
        if !self.open {
            let begin = if self.fill { D2D1_FIGURE_BEGIN_FILLED } else { D2D1_FIGURE_BEGIN_HOLLOW };
            unsafe { self.sink.BeginFigure(pt(self.start.0, self.start.1), begin) };
            self.open = true;
        }
    }
}

impl PathEmitter for SinkEmitter<'_> {
    fn move_to(&mut self, x: f64, y: f64) {
        self.finish();
        self.start = (x as f32, y as f32);
        self.ensure_open();
    }
    fn line_to(&mut self, x: f64, y: f64) {
        self.ensure_open();
        unsafe { self.sink.AddLine(pt(x as f32, y as f32)) };
    }
    fn curve_to(&mut self, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        self.ensure_open();
        unsafe {
            self.sink.AddBezier(&D2D1_BEZIER_SEGMENT {
                point1: pt(c1x as f32, c1y as f32),
                point2: pt(c2x as f32, c2y as f32),
                point3: pt(x as f32, y as f32),
            })
        };
    }
    fn close(&mut self) {
        if self.open {
            unsafe { self.sink.EndFigure(D2D1_FIGURE_END_CLOSED) };
            self.open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    //! The Direct2D renderer against the GDI+ painter it sits beside, on
    //! the same scenes built through the backend's own API.
    //!
    //! Both renderers anti-alias edges with their own rasterizers, so
    //! pixels are compared where the GDI frame is locally smooth (away from
    //! shape edges and glyphs) and, separately, in aggregate. Text can't be
    //! compared across renderers (GDI and DirectWrite rasterize glyphs
    //! differently by design); the text tests instead pin the invariant the
    //! renderer switch exists to keep — glyphs land in the box DirectWrite
    //! measured.
    //!
    //! Needs Direct2D: real Windows, or Wine-Staging + DXVK. Under a Wine
    //! without it (stock Wine on macOS can't create the Direct3D 10-level
    //! device Direct2D needs) each test prints `SKIP` and returns; on
    //! Windows a device failure fails the test.
    use super::*;
    use crate::{WindowsNode, RendererKind};
    use runtime_shared::primitives::icon::{FillRule, IconData};
    use runtime_shared::{
        AccessibilityProps, Color, Gradient, GradientKind, GradientStop, Length, Overflow, Position,
        RadialExtent, StyleRules,
    };
    use std::rc::Rc;
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
        BITMAPINFOHEADER, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DestroyWindow, WINDOW_EX_STYLE, WS_POPUP, WS_VISIBLE,
    };

    const CLIENT: (i32, i32) = (400, 300);

    fn host_window() -> HWND {
        unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(windows::core::w!("STATIC").as_ptr()),
                PCWSTR(windows::core::w!("d2d-test").as_ptr()),
                WS_POPUP | WS_VISIBLE,
                0,
                0,
                CLIENT.0,
                CLIENT.1,
                None,
                None,
                None,
                None,
            )
        }
        .expect("host window")
    }

    fn style(f: impl FnOnce(&mut StyleRules)) -> Rc<StyleRules> {
        let mut s = StyleRules::default();
        f(&mut s);
        Rc::new(s)
    }

    /// An absolutely positioned box.
    fn placed(x: f32, y: f32, w: f32, h: f32, f: impl FnOnce(&mut StyleRules)) -> Rc<StyleRules> {
        style(|s| {
            s.position = Some(Position::Absolute);
            s.left = Some(Length::Px(x).into());
            s.top = Some(Length::Px(y).into());
            s.width = Some(Length::Px(w).into());
            s.height = Some(Length::Px(h).into());
            f(s);
        })
    }

    fn bg(c: &str) -> impl FnOnce(&mut StyleRules) + '_ {
        move |s| s.background = Some(Color::from(c).into())
    }

    type Rgb = [u8; 3];

    struct Frame {
        px: Vec<Rgb>,
    }

    impl Frame {
        fn at(&self, x: i32, y: i32) -> Rgb {
            self.px[(y * CLIENT.0 + x) as usize]
        }
    }

    /// Build `scene` under `renderer` at 96 DPI, lay it out and return the
    /// frame, plus the backend for geometry queries.
    fn frame(
        renderer: RendererKind,
        scene: &dyn Fn(&mut WindowsBackend, &mut WindowsNode),
    ) -> (Frame, WindowsBackend, HWND) {
        let host = host_window();
        let mut b = WindowsBackend::with_renderer(host, renderer);
        let a11y = AccessibilityProps::default();
        let mut root = b.create_view(&a11y);
        b.apply_style(&root, &style(|s| s.background = Some(Color::from("#ffffff").into())));
        scene(&mut b, &mut root);
        b.set_dpi(96);
        b.finish(root);
        let (w, h) = CLIENT;
        let px = match renderer {
            RendererKind::D2d => {
                let bgra = b.render_d2d_offscreen(w, h).expect("d2d").expect("d2d frame");
                bgra.chunks_exact(4).map(|p| [p[2], p[1], p[0]]).collect()
            }
            RendererKind::Gdi => unsafe {
                let dc = CreateCompatibleDC(None);
                let info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w,
                        biHeight: -h,
                        biPlanes: 1,
                        biBitCount: 32,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let mut bits: *mut std::ffi::c_void = std::ptr::null_mut();
                let bmp = CreateDIBSection(Some(dc), &info, DIB_RGB_COLORS, &mut bits, None, 0).expect("DIB");
                let old = SelectObject(dc, HGDIOBJ(bmp.0));
                b.paint(dc, w, h);
                let px = std::slice::from_raw_parts(bits as *const u32, (w * h) as usize)
                    .iter()
                    .map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8])
                    .collect();
                SelectObject(dc, old);
                let _ = DeleteObject(HGDIOBJ(bmp.0));
                let _ = DeleteDC(dc);
                px
            },
        };
        (Frame { px }, b, host)
    }

    /// `false` (after printing why) when Direct2D can't run here and this is
    /// Wine — see the module docs. On Windows a failure panics.
    fn d2d_available(test: &str) -> bool {
        match D2dRenderer::new() {
            Ok(_) => true,
            Err(e) if under_wine() => {
                eprintln!("SKIP {test}: Direct2D unavailable under this Wine ({e}); use Wine-Staging + DXVK");
                false
            }
            Err(e) => panic!("{test}: Direct2D device creation failed: {e}"),
        }
    }

    fn both(scene: &dyn Fn(&mut WindowsBackend, &mut WindowsNode)) -> (Frame, Frame) {
        let (gdi, _, h1) = frame(RendererKind::Gdi, scene);
        let (d2d, _, h2) = frame(RendererKind::D2d, scene);
        unsafe {
            let _ = DestroyWindow(h1);
            let _ = DestroyWindow(h2);
        }
        (gdi, d2d)
    }

    use crate::under_wine;
    use windows::Win32::Foundation::HWND;

    fn diff(a: Rgb, b: Rgb) -> i32 {
        (0..3).map(|k| (a[k] as i32 - b[k] as i32).abs()).max().unwrap()
    }

    /// The frames agree within `tol` wherever the GDI frame is smooth
    /// (every 8-neighbour within `smooth` of the pixel — interiors and
    /// gentle gradients, not edges), and their total ink matches.
    ///
    /// Edges are compared only through the ink total because the two
    /// rasterizers sit on different pixel grids: GDI+'s default pixel
    /// offset centres pixel p at p, so an edge on a whole coordinate is
    /// half-covered on both sides (a soft 1-px seam), where Direct2D centres
    /// at p + 0.5 and draws it crisp. A half-pixel shift moves ink between
    /// neighbours without changing the total; a missing or extra shape
    /// changes it far beyond the bound.
    fn assert_agree(name: &str, gdi: &Frame, d2d: &Frame, tol: i32, smooth: i32) {
        let (w, h) = CLIENT;
        let mut checked = 0;
        let (mut ink_g, mut ink_d) = (0i64, 0i64);
        for y in 0..h {
            for x in 0..w {
                let g = gdi.at(x, y);
                let dd = d2d.at(x, y);
                ink_g += g.iter().map(|c| 255 - *c as i64).sum::<i64>();
                ink_d += dd.iter().map(|c| 255 - *c as i64).sum::<i64>();
                if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                    continue;
                }
                let flat = (-1..=1).all(|dy| (-1..=1).all(|dx| diff(gdi.at(x + dx, y + dy), g) <= smooth));
                if !flat {
                    continue;
                }
                checked += 1;
                let d = d2d.at(x, y);
                assert!(diff(g, d) <= tol, "{name}: ({x}, {y}) gdi {g:?} d2d {d:?}");
            }
        }
        assert!(checked > (w * h) as usize / 2, "{name}: only {checked} smooth pixels");
        let rel = (ink_g - ink_d).abs() as f64 / ink_g.max(1) as f64;
        assert!(rel < 0.005, "{name}: total ink differs by {:.2}% (gdi {ink_g}, d2d {ink_d})", rel * 100.0);
    }

    #[test]
    fn d2d_matches_gdi_for_fills_radii_and_borders() {
        let _serial = crate::render_test_lock();
        if !d2d_available("d2d_matches_gdi_for_fills_radii_and_borders") {
            return;
        }
        let scene = |b: &mut WindowsBackend, root: &mut WindowsNode| {
            let a11y = AccessibilityProps::default();
            let boxes: Vec<Rc<StyleRules>> = vec![
                placed(10.0, 10.0, 100.0, 50.0, bg("#ff0000")),
                placed(130.0, 10.0, 80.0, 60.0, |s| {
                    s.background = Some(Color::from("#2060ff").into());
                    for r in [&mut s.border_top_left_radius, &mut s.border_top_right_radius, &mut s.border_bottom_left_radius, &mut s.border_bottom_right_radius] {
                        *r = Some(Length::Px(12.0).into());
                    }
                }),
                // Mixed radii with a square corner (the zero-arc regression).
                placed(230.0, 10.0, 120.0, 80.0, |s| {
                    s.background = Some(Color::from("#20a040").into());
                    s.border_top_left_radius = Some(Length::Px(16.0).into());
                    s.border_bottom_right_radius = Some(Length::Px(30.0).into());
                    s.border_bottom_left_radius = Some(Length::Px(4.0).into());
                }),
                placed(10.0, 110.0, 120.0, 70.0, |s| {
                    s.background = Some(Color::from("#ffe0a0").into());
                    for r in [&mut s.border_top_left_radius, &mut s.border_top_right_radius, &mut s.border_bottom_left_radius, &mut s.border_bottom_right_radius] {
                        *r = Some(Length::Px(10.0).into());
                    }
                    for (wd, c) in [(&mut s.border_top_width, &mut s.border_top_color), (&mut s.border_right_width, &mut s.border_right_color), (&mut s.border_bottom_width, &mut s.border_bottom_color), (&mut s.border_left_width, &mut s.border_left_color)] {
                        *wd = Some(4.0.into());
                        *c = Some(Color::from("#202020").into());
                    }
                }),
                placed(160.0, 110.0, 120.0, 70.0, |s| {
                    s.border_top_width = Some(6.0.into());
                    s.border_top_color = Some(Color::from("#e01010").into());
                    s.border_left_width = Some(2.0.into());
                    s.border_left_color = Some(Color::from("#1010e0").into());
                }),
                placed(300.0, 110.0, 80.0, 80.0, |s| {
                    s.background = Some(Color::from("#000000").into());
                    s.opacity = Some(0.5.into());
                }),
            ];
            for st in boxes {
                let v = b.create_view(&a11y);
                b.apply_style(&v, &st);
                b.insert(root, v);
            }
        };
        let (gdi, d2d) = both(&scene);
        assert_eq!(d2d.at(60, 35), [255, 0, 0], "solid fill");
        assert_eq!(d2d.at(232, 88), [32, 160, 64], "square corner stays square");
        assert_agree("fills", &gdi, &d2d, 3, 2);
    }

    #[test]
    fn d2d_matches_gdi_for_linear_and_radial_gradients() {
        let _serial = crate::render_test_lock();
        if !d2d_available("d2d_matches_gdi_for_linear_and_radial_gradients") {
            return;
        }
        let scene = |b: &mut WindowsBackend, root: &mut WindowsNode| {
            let a11y = AccessibilityProps::default();
            let stops = |a: &str, z: &str| {
                vec![
                    GradientStop { offset: 0.0, color: Color::from(a) },
                    GradientStop { offset: 1.0, color: Color::from(z) },
                ]
            };
            let grads = vec![
                placed(10.0, 10.0, 180.0, 80.0, |s| {
                    s.background_gradient = Some(Gradient { kind: GradientKind::Linear { angle_deg: 90.0 }, stops: stops("#ff0000", "#0000ff") });
                }),
                placed(210.0, 10.0, 80.0, 180.0, |s| {
                    s.background_gradient = Some(Gradient { kind: GradientKind::Linear { angle_deg: 180.0 }, stops: stops("#00ff00", "#000000") });
                }),
                placed(10.0, 110.0, 160.0, 160.0, |s| {
                    s.background_gradient = Some(Gradient {
                        kind: GradientKind::Radial { center: (0.5, 0.5), radius: 1.0, extent: RadialExtent::ClosestSide },
                        stops: stops("#ffffff", "#ff8000"),
                    });
                }),
            ];
            for st in grads {
                let v = b.create_view(&a11y);
                b.apply_style(&v, &st);
                b.insert(root, v);
            }
        };
        let (gdi, d2d) = both(&scene);
        // Linear 90°: red at the left end, blue at the right, mixed between.
        assert!(d2d.at(12, 50)[0] > 240 && d2d.at(188, 50)[2] > 240, "linear ends");
        assert_agree("gradients", &gdi, &d2d, 6, 8);
    }

    #[test]
    fn d2d_clips_overflow_and_scroll_content_like_gdi() {
        let _serial = crate::render_test_lock();
        if !d2d_available("d2d_clips_overflow_and_scroll_content_like_gdi") {
            return;
        }
        let scene = |b: &mut WindowsBackend, root: &mut WindowsNode| {
            let a11y = AccessibilityProps::default();
            // A rounded overflow:hidden parent with a child twice its size.
            let mut clip = b.create_view(&a11y);
            b.apply_style(&clip, &placed(10.0, 10.0, 120.0, 120.0, |s| {
                s.overflow = Some(Overflow::Hidden);
                for r in [&mut s.border_top_left_radius, &mut s.border_top_right_radius, &mut s.border_bottom_left_radius, &mut s.border_bottom_right_radius] {
                    *r = Some(Length::Px(30.0).into());
                }
            }));
            let big = b.create_view(&a11y);
            b.apply_style(&big, &placed(-20.0, -20.0, 200.0, 200.0, bg("#8040c0")));
            b.insert(&mut clip, big);
            b.insert(root, clip);
            // A scroll view scrolled 60px down a stack of bars; bars that
            // scrolled out (and those below) are culled, the rest clipped.
            let mut scroll = b.create_scroll_view(false, None, &a11y);
            b.apply_style(&scroll, &placed(200.0, 10.0, 150.0, 100.0, |_| {}));
            let mut column = b.create_view(&a11y);
            b.apply_style(&column, &style(|s| {
                s.flex_shrink = Some(0.0.into());
                s.width = Some(Length::Px(150.0).into());
            }));
            for (i, c) in ["#ff0000", "#00c000", "#0000ff", "#c0c000", "#00c0c0", "#c000c0"].iter().enumerate() {
                let bar = b.create_view(&a11y);
                b.apply_style(&bar, &style(|s| {
                    s.height = Some(Length::Px(40.0).into());
                    s.flex_shrink = Some(0.0.into());
                    s.background = Some(Color::from(*c).into());
                }));
                let _ = i;
                b.insert(&mut column, bar);
            }
            b.insert(&mut scroll, column);
            b.set_node_scroll(&scroll, 0.0, 60.0);
            b.insert(root, scroll);
        };
        let (gdi, d2d) = both(&scene);
        // Scrolled by 60: device y 10..30 shows bar 1 (green, 40..80),
        // y 30..70 bar 2 (blue), y 70..110 bar 3 (yellow).
        assert_eq!(d2d.at(270, 20), [0, 192, 0]);
        assert_eq!(d2d.at(270, 50), [0, 0, 255]);
        assert_eq!(d2d.at(270, 100), [192, 192, 0]);
        assert_eq!(d2d.at(270, 115), [255, 255, 255], "below the viewport: clipped");
        assert_eq!(d2d.at(70, 70), [128, 64, 192]);
        // The child (200×200 at −20) is clipped to the 120×120 box.
        assert_eq!(d2d.at(135, 70), [255, 255, 255], "outside the clip box");
        if under_wine() {
            // Wine's Direct2D stubs `PushLayer`: corners stay square there
            // (see `node`), so only the box clip is checkable.
            eprintln!("note: Wine — rounded-clip corner check skipped (ID2D1DeviceContext::PushLayer is a stub)");
            return;
        }
        // The rounded clip cuts the child at the corner, not the box.
        assert_eq!(d2d.at(12, 12), [255, 255, 255], "outside the rounded corner");
        assert_agree("clips", &gdi, &d2d, 3, 2);
    }

    #[test]
    fn d2d_matches_gdi_for_filled_and_stroked_icons() {
        let _serial = crate::render_test_lock();
        if !d2d_available("d2d_matches_gdi_for_filled_and_stroked_icons") {
            return;
        }
        static RING: &[&str] = &["M12 2 A10 10 0 1 1 11.99 2 Z"];
        static SQUARE: &[&str] = &["M4 4 L20 4 L20 20 L4 20 Z"];
        let scene = |b: &mut WindowsBackend, root: &mut WindowsNode| {
            let a11y = AccessibilityProps::default();
            for (i, (paths, filled)) in [(SQUARE, true), (RING, false)].into_iter().enumerate() {
                let data = IconData { view_box: (24, 24), paths, fill_rule: FillRule::NonZero, filled };
                let icon = b.create_icon(&data, Some(&Color::from("#000000")), &a11y);
                b.apply_style(&icon, &placed(20.0 + i as f32 * 120.0, 20.0, 96.0, 96.0, |_| {}));
                b.insert(root, icon);
            }
        };
        let (gdi, d2d) = both(&scene);
        let ink = |f: &Frame, x0: i32| {
            (20..116).flat_map(|y| (x0..x0 + 96).map(move |x| (x, y))).filter(|&(x, y)| f.at(x, y)[0] < 128).count() as f64
        };
        for (x0, name) in [(20, "filled square"), (140, "stroked ring")] {
            let (g, d) = (ink(&gdi, x0), ink(&d2d, x0));
            assert!(g > 100.0, "{name}: GDI drew it");
            assert!((g - d).abs() / g < 0.06, "{name}: ink {g} (gdi) vs {d} (d2d)");
        }
        // The filled square's interior and the ring's hollow centre.
        assert_eq!(d2d.at(68, 68), [0, 0, 0]);
        assert_eq!(d2d.at(188, 68), [255, 255, 255]);
    }

    /// DirectWrite measures and DirectWrite draws: wrapped glyphs stay in
    /// the box the layout gave the text, and each measured line is inked.
    #[test]
    fn d2d_text_is_drawn_where_directwrite_measured_it() {
        let _serial = crate::render_test_lock();
        if !d2d_available("d2d_text_is_drawn_where_directwrite_measured_it") {
            return;
        }
        let scene = |b: &mut WindowsBackend, root: &mut WindowsNode| {
            let a11y = AccessibilityProps::default();
            let t = b.create_text("The quick brown fox jumps over the lazy dog again and again", &a11y);
            b.apply_style(&t, &style(|s| {
                s.position = Some(Position::Absolute);
                s.left = Some(Length::Px(20.0).into());
                s.top = Some(Length::Px(30.0).into());
                s.width = Some(Length::Px(160.0).into());
                s.font_size = Some(Length::Px(16.0).into());
                s.color = Some(Color::from("#000000").into());
            }));
            b.insert(root, t);
        };
        let (f, b, host) = frame(RendererKind::D2d, &scene);
        let text_id = b.children.get(&b.root_id.unwrap()).unwrap()[0];
        let (x, y, w, h) = b.node_frame(text_id).expect("laid out");
        let lines = match &b.nodes[&text_id].kind {
            NodeKind::Text(t) => t.lines.as_ref().expect("wrapped").lines.len(),
            _ => unreachable!(),
        };
        assert!(lines >= 3, "a 160px box wraps the sentence ({lines} lines)");
        let inked: Vec<(i32, i32)> = (0..CLIENT.1)
            .flat_map(|py| (0..CLIENT.0).map(move |px| (px, py)))
            .filter(|&(px, py)| f.at(px, py)[0] < 160)
            .collect();
        assert!(!inked.is_empty(), "text painted");
        let (ix0, iy0) = (inked.iter().map(|p| p.0).min().unwrap(), inked.iter().map(|p| p.1).min().unwrap());
        let (ix1, iy1) = (inked.iter().map(|p| p.0).max().unwrap(), inked.iter().map(|p| p.1).max().unwrap());
        // One pixel of slack for anti-aliasing at the box edge.
        assert!(ix0 as f32 >= x - 1.0 && iy0 as f32 >= y - 1.0, "ink starts inside ({ix0}, {iy0}) vs box ({x}, {y})");
        assert!(ix1 as f32 <= x + w + 1.0 && iy1 as f32 <= y + h + 1.0, "ink ends inside ({ix1}, {iy1}) vs box {w}×{h}");
        // Every line band has ink.
        let advance = h / lines as f32;
        for i in 0..lines {
            let band = (y + i as f32 * advance) as i32..(y + (i + 1) as f32 * advance) as i32;
            assert!(inked.iter().any(|p| band.contains(&p.1)), "line {i} is inked");
        }
        unsafe {
            let _ = DestroyWindow(host);
        }
    }
}
