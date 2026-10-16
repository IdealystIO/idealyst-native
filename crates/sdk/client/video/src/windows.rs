//! Windows implementation of the Video SDK — Media Foundation.
//!
//! # Surface
//!
//! A `graphics` node (`GraphicsOps::create_graphics`): a DirectComposition
//! visual in the backend's tree, which this leaf fills with a premultiplied
//! composition swapchain the size of the view (the canvas painter's setup).
//! The backend lays it out, clips and stacks it like any graphics surface.
//!
//! # URL playback
//!
//! `IMFMediaEngine` in frame-server mode: the engine decodes (on the GPU
//! when the adapter offers video decode, in software otherwise — the case
//! on a GPU-less VM) and each new frame is `TransferVideoFrame`d into the
//! swapchain's back buffer at the rect [`fit`] computes from `object_fit`
//! (`Contain` letterboxes onto transparent, `Cover` crops the source).
//! Frames are pulled on the UI thread from a `raf_loop` tick
//! (`OnVideoStreamTick`), so nothing crosses threads. Media Foundation plays
//! the formats Windows has codecs for — MP4 (H.264/HEVC/AAC), WebM/VP9 and
//! MP3 among them; HLS/DASH manifests are not (the platform's adaptive
//! streaming lives in the WinRT `MediaPlayer`, not the Media Engine).
//!
//! # Live streams
//!
//! A `MediaStream` (camera, screen capture) hands out RGBA frames; each new
//! one (`latest_pts` moved) is drawn into the swapchain with Direct2D, by
//! the same `object_fit` rule.
//!
//! # Controls
//!
//! The Media Engine has no UI. With `controls`, the leaf paints a bar along
//! the bottom of the frame — play/pause, a progress track and the time — with
//! Direct2D/DirectWrite over the video, shown while the pointer is over the
//! video or playback is paused. A press on the button toggles play; a press
//! or drag on the track seeks (the backend's mouse routing delivers to the
//! node, `InputOps::install_touch_handler`).
//!
//! The imperative ops (`play`, `pause`, `seek`, `set_muted`, `position`,
//! `duration`) go to the Media Engine; a live stream has no timeline.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use backend_windows::{WindowsBackend, WindowsNode};
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::primitives::graphics::{
    ComposedTarget, GraphicsTarget, OnLost, OnReady, OnReadyEvent, OnResize, OnResizeEvent,
};
use runtime_shared::scheduling::{raf_loop, RafLoop};
use runtime_shared::touch::{TouchEvent, TouchPhase, TouchResponse};
use runtime_vocabulary::caps::{GraphicsOps, InputOps};
use windows::core::{implement, Interface, Result, BSTR, HSTRING};
use windows::Win32::Foundation::{E_FAIL, HMODULE, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DeviceContext, ID2D1Factory1, D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
    D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_SINGLE_THREADED,
    D2D1_INTERPOLATION_MODE_LINEAR, D2D1_ROUNDED_RECT,
};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE, D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11Texture2D, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_FLAG, D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::IDCompositionVisual;
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat, DWRITE_FACTORY_TYPE_SHARED,
    DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_UNKNOWN, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::{
    IDXGIDevice, IDXGIFactory2, IDXGISurface, IDXGISwapChain1, DXGI_PRESENT, DXGI_SCALING_STRETCH,
    DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG, DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
    DXGI_USAGE_RENDER_TARGET_OUTPUT,
};
use windows::Win32::Media::MediaFoundation::{
    IMFAttributes, IMFDXGIDeviceManager, IMFMediaEngine, IMFMediaEngineClassFactory, IMFMediaEngineNotify,
    IMFMediaEngineNotify_Impl, MFCreateAttributes, MFCreateDXGIDeviceManager, MFStartup, MFARGB,
    MFSTARTUP_FULL, MFVideoNormalizedRect, MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_DXGI_MANAGER,
    MF_MEDIA_ENGINE_EVENT_ERROR, MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION,
    CLSID_MFMediaEngineClassFactory,
};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};

use crate::{MediaContent, ObjectFit, VideoOps, VideoProps};

pub(crate) static OPS: &dyn VideoOps = &WindowsVideoOps;

/// Control bar height, in logical pixels.
const BAR_H: f32 = 36.0;

runtime_tls::thread_local! {
    /// node id → player, for the imperative ops.
    static PLAYERS: RefCell<HashMap<u64, Rc<RefCell<Player>>>> = RefCell::new(HashMap::new());
}

/// One mounted video.
struct Player {
    props: Rc<VideoProps>,
    content: Content,
    gpu: Option<Gpu>,
    surface: Option<Surface>,
    binding: Option<Binding>,
    hovered: bool,
    scrubbing: bool,
    /// The visible frame is stale (a new video frame, a control change).
    dirty: bool,
    tick: Option<RafLoop>,
}

enum Content {
    None,
    Url { engine: IMFMediaEngine, last_pts: i64 },
    Stream { stream: media_stream::MediaStream, last_pts: Option<u64>, frame: Vec<u8>, size: (u32, u32) },
}

struct Binding {
    visual: IDCompositionVisual,
    composed: Arc<dyn ComposedTarget>,
    size: (u32, u32),
    scale: f32,
}

struct Gpu {
    d3d: ID3D11Device,
    ctx: ID2D1DeviceContext,
    dwrite: IDWriteFactory,
    manager: IMFDXGIDeviceManager,
}

struct Surface {
    swap: IDXGISwapChain1,
    size: (u32, u32),
}

/// Build the graphics node for one `Video` payload.
pub(crate) fn build(props: &Rc<VideoProps>, backend: &Rc<RefCell<WindowsBackend>>) -> WindowsNode {
    // Media Foundation's engine is a COM object; this thread is the UI
    // thread's single-threaded apartment (already-initialized is fine).
    let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    let player = Rc::new(RefCell::new(Player {
        props: props.clone(),
        content: Content::None,
        gpu: None,
        surface: None,
        binding: None,
        hovered: false,
        scrubbing: false,
        dirty: true,
        tick: None,
    }));
    let on_ready: OnReady = {
        let p = player.clone();
        Box::new(move |ev: OnReadyEvent| ready(&p, ev))
    };
    let on_resize: OnResize = {
        let p = player.clone();
        Box::new(move |ev: OnResizeEvent| {
            let mut pl = p.borrow_mut();
            if let Some(b) = &mut pl.binding {
                b.size = ev.size;
                b.scale = ev.scale;
            }
            pl.dirty = true;
        })
    };
    let on_lost: OnLost = {
        let p = player.clone();
        Box::new(move || {
            let mut pl = p.borrow_mut();
            pl.surface = None;
            pl.binding = None;
        })
    };
    let node = GraphicsOps::create_graphics(
        &mut *backend.borrow_mut(),
        on_ready,
        on_resize,
        on_lost,
        &AccessibilityProps::default(),
    );
    let id = node.id();
    PLAYERS.with(|m| m.borrow_mut().insert(id, player.clone()));

    if props.controls {
        let p = player.clone();
        backend.borrow_mut().install_touch_handler(&node, Rc::new(move |ev: &TouchEvent| touch(&p, ev)));
        let p = player.clone();
        backend.borrow_mut().install_hover_handler(
            &node,
            Rc::new(move |inside: bool| {
                let mut pl = p.borrow_mut();
                pl.hovered = inside;
                pl.dirty = true;
            }),
        );
    }

    // Reactive source: resolved inside the effect so a source reading
    // signals re-populates the view when they change.
    let p = player.clone();
    let props_for_effect = props.clone();
    runtime_world::effect(move || {
        let content = props_for_effect.source.resolve();
        set_content(&p, content);
    });

    // One frame pull per display tick, on this thread.
    let p = Rc::downgrade(&player);
    player.borrow_mut().tick = Some(raf_loop(move || {
        if let Some(p) = p.upgrade() {
            frame(&p);
        }
    }));

    runtime_vocabulary::style_attach::on_teardown(move || {
        PLAYERS.with(|m| m.borrow_mut().remove(&id));
        let mut pl = player.borrow_mut();
        pl.tick = None;
        if let Content::Url { engine, .. } = std::mem::replace(&mut pl.content, Content::None) {
            let _ = unsafe { engine.Shutdown() };
        }
    });
    node
}

fn ready(player: &Rc<RefCell<Player>>, ev: OnReadyEvent) {
    let GraphicsTarget::RawWindow(surface) = ev.target else { return };
    let Some(composed) = surface.composed_target().cloned() else {
        eprintln!("[video] Windows video: the graphics surface is not a composition visual");
        return;
    };
    // SAFETY: the provider holds the visual for the surface's lifetime;
    // `cloned` adds our own reference.
    let Some(visual) = (unsafe { IDCompositionVisual::from_raw_borrowed(&composed.visual().as_ptr()) }).cloned()
    else {
        return;
    };
    let mut p = player.borrow_mut();
    p.binding = Some(Binding { visual, composed, size: ev.size, scale: ev.scale });
    p.surface = None;
    p.dirty = true;
}

fn set_content(player: &Rc<RefCell<Player>>, content: MediaContent) {
    let mut p = player.borrow_mut();
    if let Content::Url { engine, .. } = std::mem::replace(&mut p.content, Content::None) {
        let _ = unsafe { engine.Shutdown() };
    }
    p.dirty = true;
    match content {
        MediaContent::None => {}
        MediaContent::Stream(stream) => {
            p.content = Content::Stream { stream, last_pts: None, frame: Vec::new(), size: (0, 0) };
        }
        MediaContent::Url(url) => {
            if p.gpu.is_none() {
                match unsafe { create_gpu() } {
                    Ok(g) => p.gpu = Some(g),
                    Err(e) => return eprintln!("[video] no Direct3D device for playback: {e}"),
                }
            }
            let props = p.props.clone();
            let manager = p.gpu.as_ref().expect("just created").manager.clone();
            match unsafe { create_engine(&manager, &url, &props) } {
                Ok(engine) => p.content = Content::Url { engine, last_pts: -1 },
                Err(e) => eprintln!("[video] could not open {url:?}: {e}"),
            }
        }
    }
}

/// One display tick: present a new frame if there is one, or the controls
/// if they changed.
fn frame(player: &Rc<RefCell<Player>>) {
    let mut p = player.borrow_mut();
    let controls = p.props.controls;
    let new_frame = match &mut p.content {
        Content::Url { engine, last_pts } => unsafe {
            // The stream's current presentation time; it moves when the
            // engine has a frame newer than the one shown.
            let fresh = match engine.OnVideoStreamTick() {
                Ok(pts) if pts != *last_pts && engine.HasVideo().as_bool() => {
                    *last_pts = pts;
                    true
                }
                _ => false,
            };
            // The time readout moves while playing.
            fresh || (controls && !engine.IsPaused().as_bool())
        },
        Content::Stream { stream, last_pts, frame, size } => {
            let pts = stream.latest_pts();
            if pts.is_some() && pts != *last_pts {
                *last_pts = pts;
                if let Some(s) = stream.latest(frame) {
                    *size = s;
                }
                true
            } else {
                false
            }
        }
        Content::None => false,
    };
    if !(new_frame || p.dirty) {
        return;
    }
    p.dirty = false;
    if let Err(e) = present(&mut p) {
        eprintln!("[video] frame failed: {e}");
        p.surface = None;
    }
}

fn present(p: &mut Player) -> Result<()> {
    let Some(binding) = &p.binding else { return Ok(()) };
    let size = (binding.size.0.max(1), binding.size.1.max(1));
    let scale = binding.scale;
    if p.gpu.is_none() {
        p.gpu = Some(unsafe { create_gpu()? });
    }
    let gpu = p.gpu.as_ref().expect("just created");
    if p.surface.as_ref().map(|s| s.size) != Some(size) {
        match &mut p.surface {
            Some(s) => unsafe {
                gpu.ctx.SetTarget(None);
                s.swap.ResizeBuffers(0, size.0, size.1, DXGI_FORMAT_UNKNOWN, DXGI_SWAP_CHAIN_FLAG(0))?;
                s.size = size;
            },
            None => p.surface = Some(unsafe { create_surface(gpu, binding, size)? }),
        }
    }
    let surface = p.surface.as_ref().expect("just made");
    unsafe {
        let back: ID3D11Texture2D = surface.swap.GetBuffer(0)?;
        let target = gpu.ctx.CreateBitmapFromDxgiSurface(&back.cast::<IDXGISurface>()?, Some(&target_props()))?;
        gpu.ctx.SetTarget(&target);
        gpu.ctx.BeginDraw();
        gpu.ctx.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.0 }));
        let drew = gpu.ctx.EndDraw(None, None);
        drew?;

        let fit_mode = p.props.object_fit;
        match &p.content {
            // Transferring before the first frame is decoded fails (E_FAIL):
            // a redraw for the controls can come first.
            Content::Url { engine, last_pts } if *last_pts >= 0 && engine.HasVideo().as_bool() => {
                let (mut vw, mut vh) = (0u32, 0u32);
                engine.GetNativeVideoSize(Some(&mut vw), Some(&mut vh))?;
                if vw > 0 && vh > 0 {
                    let (src, dst) = fit((vw as f32, vh as f32), (size.0 as f32, size.1 as f32), fit_mode);
                    let border = MFARGB { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbAlpha: 0 };
                    let dst = RECT { left: dst.0 as i32, top: dst.1 as i32, right: dst.2 as i32, bottom: dst.3 as i32 };
                    engine.TransferVideoFrame(&back, Some(&src), &dst, Some(&border))?;
                }
            }
            Content::Stream { frame, size: (fw, fh), .. } if *fw > 0 && *fh > 0 && frame.len() >= (*fw * *fh * 4) as usize => {
                let bgra = rgba_to_bgra_premultiplied(frame);
                let bitmap = gpu.ctx.CreateBitmap(
                    D2D_SIZE_U { width: *fw, height: *fh },
                    Some(bgra.as_ptr() as *const _),
                    fw * 4,
                    &D2D1_BITMAP_PROPERTIES1 {
                        pixelFormat: pixel_format(),
                        dpiX: 96.0,
                        dpiY: 96.0,
                        bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                        colorContext: std::mem::ManuallyDrop::new(None),
                    },
                )?;
                let (src, dst) = fit((*fw as f32, *fh as f32), (size.0 as f32, size.1 as f32), fit_mode);
                gpu.ctx.BeginDraw();
                gpu.ctx.DrawBitmap(
                    &bitmap,
                    Some(&D2D_RECT_F { left: dst.0, top: dst.1, right: dst.2, bottom: dst.3 }),
                    1.0,
                    D2D1_INTERPOLATION_MODE_LINEAR,
                    Some(&D2D_RECT_F {
                        left: src.left * *fw as f32,
                        top: src.top * *fh as f32,
                        right: src.right * *fw as f32,
                        bottom: src.bottom * *fh as f32,
                    }),
                    None,
                );
                gpu.ctx.EndDraw(None, None)?;
            }
            _ => {}
        }

        if p.props.controls {
            if let Content::Url { engine, .. } = &p.content {
                if p.hovered || engine.IsPaused().as_bool() || p.scrubbing {
                    draw_controls(gpu, engine, (size.0 as f32, size.1 as f32), scale)?;
                }
            }
        }
        gpu.ctx.SetTarget(None);
        surface.swap.Present(1, DXGI_PRESENT(0)).ok()?;
    }
    Ok(())
}

/// The source rect (normalized) and destination rect (pixels) that fit a
/// `content`-sized frame into a `bounds`-sized box under `mode`.
fn fit(content: (f32, f32), bounds: (f32, f32), mode: ObjectFit) -> (MFVideoNormalizedRect, (f32, f32, f32, f32)) {
    let full = MFVideoNormalizedRect { left: 0.0, top: 0.0, right: 1.0, bottom: 1.0 };
    let (cw, ch) = content;
    let (bw, bh) = bounds;
    match mode {
        ObjectFit::Cover => {
            // Fill the box; crop the overflow, centered.
            let s = (bw / cw).max(bh / ch);
            let (vis_w, vis_h) = (bw / s / cw, bh / s / ch);
            let (x0, y0) = ((1.0 - vis_w) / 2.0, (1.0 - vis_h) / 2.0);
            (MFVideoNormalizedRect { left: x0, top: y0, right: x0 + vis_w, bottom: y0 + vis_h }, (0.0, 0.0, bw, bh))
        }
        ObjectFit::Contain => {
            // The whole frame, centered, letterboxed.
            let s = (bw / cw).min(bh / ch);
            let (w, h) = (cw * s, ch * s);
            let (x, y) = ((bw - w) / 2.0, (bh - h) / 2.0);
            (full, (x.round(), y.round(), (x + w).round(), (y + h).round()))
        }
    }
}

/// The control bar's hit regions, in pixels: (play button, track).
fn bar_layout(size: (f32, f32), scale: f32) -> ((f32, f32, f32, f32), (f32, f32, f32, f32)) {
    let h = BAR_H * scale;
    let top = size.1 - h;
    let button = (0.0, top, h, size.1);
    let pad = 12.0 * scale;
    let time_w = 96.0 * scale;
    let track = (h + pad * 0.5, top, (size.0 - time_w - pad).max(h + pad), size.1);
    (button, track)
}

fn inside(r: (f32, f32, f32, f32), x: f32, y: f32) -> bool {
    x >= r.0 && x < r.2 && y >= r.1 && y < r.3
}

fn touch(player: &Rc<RefCell<Player>>, ev: &TouchEvent) -> TouchResponse {
    let mut p = player.borrow_mut();
    let Some(binding) = &p.binding else { return TouchResponse::IGNORED };
    let scale = binding.scale;
    let size = (binding.size.0 as f32, binding.size.1 as f32);
    let (x, y) = (ev.position.x * scale, ev.position.y * scale);
    let (button, track) = bar_layout(size, scale);
    let Content::Url { engine, .. } = &p.content else { return TouchResponse::IGNORED };
    let engine = engine.clone();
    let seek_to = |x: f32| unsafe {
        let frac = ((x - track.0) / (track.2 - track.0)).clamp(0.0, 1.0) as f64;
        let d = engine.GetDuration();
        if d.is_finite() && d > 0.0 {
            let _ = engine.SetCurrentTime(frac * d);
        }
    };
    let response = match ev.phase {
        TouchPhase::Began if inside(button, x, y) => {
            unsafe {
                let _ = if engine.IsPaused().as_bool() { engine.Play() } else { engine.Pause() };
            }
            TouchResponse::CONSUMED
        }
        TouchPhase::Began if inside(track, x, y) || (y >= track.1 && x >= track.0) => {
            p.scrubbing = true;
            seek_to(x);
            TouchResponse::CLAIMED
        }
        TouchPhase::Moved if p.scrubbing => {
            seek_to(x);
            TouchResponse::CONSUMED
        }
        TouchPhase::Ended | TouchPhase::Cancelled if p.scrubbing => {
            p.scrubbing = false;
            TouchResponse::CONSUMED
        }
        _ => TouchResponse::IGNORED,
    };
    p.dirty = true;
    response
}

unsafe fn draw_controls(gpu: &Gpu, engine: &IMFMediaEngine, size: (f32, f32), scale: f32) -> Result<()> {
    let ctx = &gpu.ctx;
    let (button, track) = bar_layout(size, scale);
    let white = ctx.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 1.0 }, None)?;
    let dim = ctx.CreateSolidColorBrush(&D2D1_COLOR_F { r: 1.0, g: 1.0, b: 1.0, a: 0.35 }, None)?;
    let shade = ctx.CreateSolidColorBrush(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 0.55 }, None)?;
    ctx.BeginDraw();
    ctx.FillRectangle(&D2D_RECT_F { left: 0.0, top: button.1, right: size.0, bottom: size.1 }, &shade);
    // Play triangle, or two pause bars.
    let (cx, cy, r) = ((button.0 + button.2) / 2.0, (button.1 + button.3) / 2.0, 7.0 * scale);
    if engine.IsPaused().as_bool() {
        let factory: ID2D1Factory1 = ctx.GetFactory()?.cast()?;
        let path = factory.CreatePathGeometry()?;
        let sink = path.Open()?;
        use windows::Win32::Graphics::Direct2D::Common::{D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED};
        use windows_numerics::Vector2;
        sink.BeginFigure(Vector2 { X: cx - r * 0.6, Y: cy - r }, D2D1_FIGURE_BEGIN_FILLED);
        sink.AddLine(Vector2 { X: cx + r, Y: cy });
        sink.AddLine(Vector2 { X: cx - r * 0.6, Y: cy + r });
        sink.EndFigure(D2D1_FIGURE_END_CLOSED);
        sink.Close()?;
        ctx.FillGeometry(&path, &white, None);
    } else {
        let w = r * 0.55;
        for x0 in [cx - r * 0.8, cx + r * 0.25] {
            ctx.FillRectangle(&D2D_RECT_F { left: x0, top: cy - r, right: x0 + w, bottom: cy + r }, &white);
        }
    }
    // Progress track.
    let (pos, dur) = (engine.GetCurrentTime(), engine.GetDuration());
    let frac = if dur.is_finite() && dur > 0.0 { (pos / dur).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let ty = (track.1 + track.3) / 2.0;
    let th = 2.0 * scale;
    let rr = |l: f32, r: f32| D2D1_ROUNDED_RECT {
        rect: D2D_RECT_F { left: l, top: ty - th, right: r, bottom: ty + th },
        radiusX: th,
        radiusY: th,
    };
    ctx.FillRoundedRectangle(&rr(track.0, track.2), &dim);
    ctx.FillRoundedRectangle(&rr(track.0, track.0 + (track.2 - track.0) * frac), &white);
    // Time readout.
    let label: Vec<u16> = format!("{} / {}", clock(pos), clock(dur)).encode_utf16().collect();
    let format = text_format(&gpu.dwrite, 12.0 * scale)?;
    ctx.DrawText(
        &label,
        &format,
        &D2D_RECT_F { left: track.2 + 8.0 * scale, top: button.1, right: size.0, bottom: size.1 },
        &white,
        D2D1_DRAW_TEXT_OPTIONS_NONE,
        DWRITE_MEASURING_MODE_NATURAL,
    );
    ctx.EndDraw(None, None)
}

/// `m:ss` (or `h:mm:ss`); `--:--` for an unknown duration.
fn clock(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "--:--".to_string();
    }
    let s = seconds as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

unsafe fn text_format(dwrite: &IDWriteFactory, size: f32) -> Result<IDWriteTextFormat> {
    let f = dwrite.CreateTextFormat(
        &HSTRING::from("Segoe UI"),
        None,
        DWRITE_FONT_WEIGHT_SEMI_BOLD,
        DWRITE_FONT_STYLE_NORMAL,
        DWRITE_FONT_STRETCH_NORMAL,
        size,
        &HSTRING::from(""),
    )?;
    f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
    Ok(f)
}

/// Straight RGBA → premultiplied BGRA (Direct2D's bitmap format).
fn rgba_to_bgra_premultiplied(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for px in rgba.chunks_exact(4) {
        let a = px[3] as u16;
        let pm = |c: u8| ((c as u16 * a + 127) / 255) as u8;
        out.extend_from_slice(&[pm(px[2]), pm(px[1]), pm(px[0]), px[3]]);
    }
    out
}

fn pixel_format() -> D2D1_PIXEL_FORMAT {
    D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED }
}

fn target_props() -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: pixel_format(),
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
        colorContext: std::mem::ManuallyDrop::new(None),
    }
}

/// A Direct3D device the Media Engine can decode on (video support where
/// the adapter has it; a plain device and software decode where it does
/// not — WARP), with Direct2D/DirectWrite on top and Media Foundation's
/// DXGI device manager around it.
unsafe fn create_gpu() -> Result<Gpu> {
    let attempts: [(D3D_DRIVER_TYPE, D3D11_CREATE_DEVICE_FLAG); 3] = [
        (D3D_DRIVER_TYPE_HARDWARE, D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT),
        (D3D_DRIVER_TYPE_HARDWARE, D3D11_CREATE_DEVICE_BGRA_SUPPORT),
        (D3D_DRIVER_TYPE_WARP, D3D11_CREATE_DEVICE_BGRA_SUPPORT),
    ];
    let mut d3d = None;
    for (driver, flags) in attempts {
        let mut dev: Option<ID3D11Device> = None;
        if D3D11CreateDevice(None, driver, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut dev), None, None)
            .is_ok()
        {
            d3d = dev;
            break;
        }
    }
    let d3d = d3d.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
    // The engine decodes on worker threads against this device.
    if let Ok(mt) = d3d.cast::<windows::Win32::Graphics::Direct3D10::ID3D10Multithread>() {
        let _ = mt.SetMultithreadProtected(true);
    }
    MFStartup(MF_VERSION, MFSTARTUP_FULL)?;
    let mut token = 0u32;
    let mut manager = None;
    MFCreateDXGIDeviceManager(&mut token, &mut manager)?;
    let manager = manager.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
    manager.ResetDevice(&d3d, token)?;
    let factory: ID2D1Factory1 = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
    let device = factory.CreateDevice(&d3d.cast::<IDXGIDevice>()?)?;
    let ctx = device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
    let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
    Ok(Gpu { d3d, ctx, dwrite, manager })
}

unsafe fn create_surface(gpu: &Gpu, binding: &Binding, size: (u32, u32)) -> Result<Surface> {
    let factory: IDXGIFactory2 = gpu.d3d.cast::<IDXGIDevice>()?.GetAdapter()?.GetParent()?;
    let desc = DXGI_SWAP_CHAIN_DESC1 {
        Width: size.0,
        Height: size.1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
        BufferCount: 2,
        Scaling: DXGI_SCALING_STRETCH,
        SwapEffect: DXGI_SWAP_EFFECT_FLIP_SEQUENTIAL,
        // Letterbox bars are transparent over the scene behind.
        AlphaMode: DXGI_ALPHA_MODE_PREMULTIPLIED,
        ..Default::default()
    };
    let swap = factory.CreateSwapChainForComposition(&gpu.d3d, &desc, None)?;
    binding.visual.SetContent(&swap)?;
    // Swapchain content appears at the tree's next commit.
    binding.composed.commit();
    Ok(Surface { swap, size })
}

/// Required by the Media Engine; it reports state changes here (on its own
/// threads). The leaf polls state on the UI thread instead, so only errors
/// are noted, for the log.
#[implement(IMFMediaEngineNotify)]
struct Notify;

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, param1: usize, param2: u32) -> Result<()> {
        if event == MF_MEDIA_ENGINE_EVENT_ERROR.0 as u32 {
            eprintln!("[video] Media Engine error {param1} (HRESULT {param2:#010x})");
        }
        Ok(())
    }
}

unsafe fn create_engine(manager: &IMFDXGIDeviceManager, url: &str, props: &VideoProps) -> Result<IMFMediaEngine> {
    let mut attrs: Option<IMFAttributes> = None;
    MFCreateAttributes(&mut attrs, 3)?;
    let attrs = attrs.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
    let notify: IMFMediaEngineNotify = Notify.into();
    attrs.SetUnknown(&MF_MEDIA_ENGINE_DXGI_MANAGER, manager)?;
    attrs.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)?;
    attrs.SetUINT32(&MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32)?;
    let factory: IMFMediaEngineClassFactory = CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
    let engine = factory.CreateInstance(0, &attrs)?;
    engine.SetMuted(props.muted)?;
    engine.SetLoop(props.loop_playback)?;
    engine.SetAutoPlay(props.autoplay)?;
    engine.SetSource(&BSTR::from(url))?;
    Ok(engine)
}

struct WindowsVideoOps;

impl WindowsVideoOps {
    fn engine(node: &dyn std::any::Any) -> Option<IMFMediaEngine> {
        let id = node.downcast_ref::<WindowsNode>()?.id();
        PLAYERS.with(|m| {
            let p = m.borrow().get(&id)?.clone();
            let p = p.borrow();
            match &p.content {
                Content::Url { engine, .. } => Some(engine.clone()),
                _ => None,
            }
        })
    }
}

impl VideoOps for WindowsVideoOps {
    fn play(&self, node: &dyn std::any::Any) {
        if let Some(e) = Self::engine(node) {
            let _ = unsafe { e.Play() };
        }
    }
    fn pause(&self, node: &dyn std::any::Any) {
        if let Some(e) = Self::engine(node) {
            let _ = unsafe { e.Pause() };
        }
    }
    fn seek(&self, node: &dyn std::any::Any, seconds: f32) {
        if let Some(e) = Self::engine(node) {
            let _ = unsafe { e.SetCurrentTime(seconds as f64) };
        }
    }
    fn set_muted(&self, node: &dyn std::any::Any, muted: bool) {
        if let Some(e) = Self::engine(node) {
            let _ = unsafe { e.SetMuted(muted) };
        }
    }
    fn position(&self, node: &dyn std::any::Any) -> f32 {
        Self::engine(node).map(|e| unsafe { e.GetCurrentTime() } as f32).unwrap_or(0.0)
    }
    fn duration(&self, node: &dyn std::any::Any) -> f32 {
        Self::engine(node)
            .map(|e| unsafe { e.GetDuration() })
            .filter(|d| d.is_finite())
            .map(|d| d as f32)
            .unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_letterboxes_and_cover_crops() {
        // 16:9 frame in a square box.
        let (src, dst) = fit((1600.0, 900.0), (400.0, 400.0), ObjectFit::Contain);
        assert_eq!((src.left, src.right), (0.0, 1.0));
        assert_eq!(dst, (0.0, 88.0, 400.0, 313.0));
        let (src, dst) = fit((1600.0, 900.0), (400.0, 400.0), ObjectFit::Cover);
        assert_eq!(dst, (0.0, 0.0, 400.0, 400.0));
        assert!((src.right - src.left - 0.5625).abs() < 1e-4, "{:?}", (src.left, src.right));
        assert_eq!((src.top, src.bottom), (0.0, 1.0));
    }

    #[test]
    fn clock_formats_minutes_and_hours() {
        assert_eq!(clock(5.4), "0:05");
        assert_eq!(clock(125.0), "2:05");
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(clock(f64::NAN), "--:--");
    }

    #[test]
    fn rgba_converts_to_premultiplied_bgra() {
        assert_eq!(rgba_to_bgra_premultiplied(&[255, 128, 0, 255, 255, 255, 255, 0]), [0, 128, 255, 255, 0, 0, 0, 0]);
    }
}
