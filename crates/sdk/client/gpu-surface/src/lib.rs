//! `gpu-surface` — the wgpu bring-up, frame target and present path shared by
//! the framework's GPU renderers (`canvas-vello`, `canvas3d-wgpu`).
//!
//! A renderer mounts through `mount` (native: blocking bring-up; web: async,
//! rAF-paced): it supplies a `paint` closure (run inside a reactive effect, so
//! any signal it reads repaints the view) and a builder that turns a ready
//! [`GpuSurface`] into its own state. The renderer then draws each frame into
//! [`GpuSurface::target_view`] and hands it back with
//! [`GpuSurface::begin_present`] / [`GpuSurface::finish_present`].
//!
//! # The frame target
//!
//! Every renderer draws into one `Rgba8Unorm` texture ([`TARGET_FORMAT`]) that
//! holds **sRGB-encoded** bytes: shaders encode before writing, and the surface
//! is chosen non-sRGB so the present blit copies the bytes verbatim (an sRGB
//! surface would encode them a second time and wash colours out). One target
//! format means passes written for one renderer (the [`OverlayCompositor`])
//! work for all of them, and self-capture reads the same bytes the screen shows.
//!
//! The target's **alpha convention** is the renderer's choice, declared once as
//! a [`FrameAlpha`]: vello stores straight alpha (its fine shader divides by
//! alpha on store), a forward 3D renderer naturally produces premultiplied
//! colour. The present blit converts to what the surface is configured for
//! (`PreMultiplied` wherever the platform offers it, since the view composites
//! over the UI behind it), so neither renderer has to fake the other's
//! convention.

#![allow(missing_docs)]

mod config;
pub use config::{
    choose_alpha_mode, choose_surface_format, check_adapter, AdapterFacts, Requirements,
    Unsupported,
};

mod present;
pub use present::{surface_wants_premultiplied, PresentBlit};

mod compose;
pub use compose::OverlayCompositor;

#[cfg(not(target_arch = "wasm32"))]
mod readback;
#[cfg(not(target_arch = "wasm32"))]
pub use readback::{headless_device, headless_device_with_limits, read_target_rgba, RenderedImage};

#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(not(target_arch = "wasm32"))]
pub use native::{adapter_meets_default, mount, GpuSurface, PresentFrame, SurfaceState};

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::{
    canvas_from_surface, claim_webgl, debug_flag, drive, marker, mount, web_dpr, webgpu_present,
    BuildFuture, GpuBackend, GpuSurface, PresentFrame, RenderFn, SurfaceState, WebGpuProbe,
};

/// Format of the frame target every renderer draws into. Non-sRGB on purpose:
/// it stores already-sRGB-encoded bytes (see the crate docs).
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// How the frame target's colour relates to its alpha. Declared by the
/// renderer that fills the target; the present blit uses it to produce what
/// the surface expects.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameAlpha {
    /// `rgb` is the colour independent of coverage (vello's storage format).
    Straight,
    /// `rgb` is already multiplied by `a` (blended 3D output).
    Premultiplied,
}

/// Texture usages for a frame-sized target on `device`.
///
/// `STORAGE_BINDING` (vello compute-writes the target) is included only when
/// the device can bind storage textures at all — WebGL2 has none, and
/// requesting the usage there fails texture creation even for a renderer that
/// never writes it from a compute shader. `COPY_SRC` (capture / readback),
/// `COPY_DST` (CPU-rasterized content uploaded into an overlay target),
/// `TEXTURE_BINDING` (present blit, compositors) and `RENDER_ATTACHMENT`
/// (render passes) are always present.
pub fn target_usages(device: &wgpu::Device) -> wgpu::TextureUsages {
    let mut usage = wgpu::TextureUsages::TEXTURE_BINDING
        | wgpu::TextureUsages::COPY_SRC
        | wgpu::TextureUsages::COPY_DST
        | wgpu::TextureUsages::RENDER_ATTACHMENT;
    if device.limits().max_storage_textures_per_shader_stage > 0 {
        usage |= wgpu::TextureUsages::STORAGE_BINDING;
    }
    usage
}

/// Create a `w`×`h` [`TARGET_FORMAT`] texture with [`target_usages`] and its
/// default view. Used for the frame target and for any same-sized
/// intermediate a renderer composites from (an overlay, a cached layer).
pub fn make_target(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: target_usages(device),
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    (texture, view)
}

/// Clear `view` to transparent in its own pass (no draw).
pub fn clear_to_transparent(encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView) {
    let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("gpu-surface-clear"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
}
