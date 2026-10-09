//! `canvas-vello` — GPU renderer for the `canvas` SDK.
//!
//! Renders a `canvas_core::Scene` with [`vello`] (GPU-compute 2D) onto the
//! framework's `graphics` primitive surface via `wgpu`. Selected over
//! `canvas-native` by passing [`register`] to the boot entry's registry
//! seam (it installs a scene handler for `canvas_core::CanvasPrim`;
//! last registration for a payload wins).
//!
//! vello needs compute shaders (Metal / Vulkan / DX12 / WebGPU). On native
//! backends that's always available; on **web** it requires WebGPU, which is
//! not universal — so the web renderer ([`render_web`]) decides per canvas at
//! runtime: a headless WebGPU probe in `on_ready` either commits the canvas to
//! webgpu+vello or falls back to Canvas2D (`canvas-native`'s rasterizer) on the
//! same element. See `render_web.rs` for why the fallback is per-canvas and
//! in-place (web binds a `<canvas>` to its first context type permanently, and
//! the web backend can't swap a mounted node).
//!
//! A single generic [`register`] covers every host: the GPU surface is
//! obtained from `runtime_vocabulary::caps::GraphicsOps::create_graphics`,
//! so no per-platform module is needed (unlike `canvas-native`).
#![allow(missing_docs)]

/// Name used in this renderer's logs and console markers.
pub(crate) const LABEL: &str = "canvas-vello";

/// What an adapter must offer for vello's GPU-driven compute pipeline: indirect
/// dispatch (the iOS Simulator's Metal lacks it) and, on Vulkan, explicit f16
/// for the `flatten` shader (the Android emulator's Vulkan lacks it). A GPU
/// without these leaves the payload to `canvas-native`.
pub(crate) const VELLO_REQUIREMENTS: gpu_surface::Requirements = gpu_surface::Requirements {
    downlevel: wgpu::DownlevelFlags::INDIRECT_EXECUTION,
    f16_on_vulkan: true,
};

// Overscan for cached layers (env-tuned), shared by the native and web paths.
mod overscan;

// Canvas scenes rendered with vello into a texture on a CALLER-owned device —
// for embedding 2D canvas content in another GPU renderer's frame (the
// `canvas3d` overlay). Also home of the shared `Renderer` constructor.
mod scene_renderer;
pub use scene_renderer::{adapter_can_run_vello, SceneRenderer};

// The same op list rasterized on the CPU with vello_cpu, for devices that
// can't run vello's compute pipeline (WebGL2, simulators) but still need the
// content in a GPU texture. Opt-in: only `canvas3d-wgpu` needs it.
#[cfg(feature = "cpu")]
mod encode_cpu;
#[cfg(feature = "cpu")]
pub use encode_cpu::{cpu_footprint, rasterize_cpu, rasterize_cpu_region, Footprint, PixelRect};

/// The source-over compositor canvas-vello uses everywhere it lays vello
/// content over what the frame target holds: both sides straight alpha (vello's
/// storage convention; exact over the opaque backdrops it composites onto).
pub(crate) fn overlay_compositor(device: &wgpu::Device) -> gpu_surface::OverlayCompositor {
    gpu_surface::OverlayCompositor::new(
        device,
        gpu_surface::FrameAlpha::Straight,
        gpu_surface::FrameAlpha::Straight,
    )
}

// Scene→vello translation, shared by the native and web renderers (no GPU or
// async — pure op-list walk). Identical output across targets (CLAUDE.md §7).
mod encode;

// Animated-image override textures (the renderer half of encode's stable-handle
// scheme for `generation > 0` sources — the video frame pump).
mod anim;

// Native renderer: blocking wgpu init. macOS + iOS (objc2 0.6 Metal coexists
// with the framework's 0.2 — see Cargo.toml; iOS sim/devices use host Metal,
// which has f16/compute), Android (Vulkan), and desktop Linux/Windows.
#[cfg(not(target_arch = "wasm32"))]
mod render;
#[cfg(not(target_arch = "wasm32"))]
pub use render::register;
// Headless offscreen export — render a `canvas::Scene` to RGBA8 with no surface
// (server-side thumbnails / image export). GPU with software fallback.
#[cfg(not(target_arch = "wasm32"))]
pub use render::{render_to_rgba, RenderedImage};

// Surface-less persistent compositor — the engine the `video-compose` SDK drives
// to transform one `MediaStream` into another (scene + layers → offscreen target
// → output stream / CPU read-back). Reuses the on-screen renderer's compositor
// passes without a window.
#[cfg(not(target_arch = "wasm32"))]
mod headless;
#[cfg(not(target_arch = "wasm32"))]
pub use headless::HeadlessCompositor;

// Scene classification (`ScenePlan` / `plan_scene`) for the instanced fast path,
// shared by the native and web renderers (pure op-list walk, no GPU).
mod plan;

// Instanced analytic-shape (rounded-box SDF) fast path for shape-batch scenes —
// the throughput path for a `DrawOp::Shapes` grid/scatter. Drives both a PURE
// shape scene (the whole frame is the instanced pass) and a HYBRID scene whose
// leading ops are shapes (an instanced backdrop, then vello over the top — see
// `compose`). Pure wgpu + `canvas_core`, so it serves both the native renderer
// and the web WebGPU renderer (`render_web`), which now composites texture layers
// itself instead of punting layered canvases to Canvas2D.
mod shape_pass;

// Compositing a scene's texture runs (layers at their `DrawOp::Texture`
// position, vector ops after them on top) — the frame model shared by the
// native, web and headless renderers, with its GPU-ordering rule.
mod texture_runs;

// Transformed-quad compositor: composites a cached layer texture under a camera
// affine (the `DrawOp::LayerCached` fast path). Shared by the native and web
// renderers. Pure wgpu + `canvas_core`, so it isn't wasm-gated.
mod compose_transform;

// The texture-layer blit (shader + crop/fit geometry + uniform slots) shared by
// the GPU layer compositors: macOS, Linux and web. Other native targets use the
// no-op stub compositor.
#[cfg(any(target_os = "macos", target_os = "linux", target_arch = "wasm32"))]
mod layer_blit;

// Web renderer: async wgpu init over the browser's WebGPU backend, with a
// per-canvas Canvas2D fallback when WebGPU is unavailable.
#[cfg(target_arch = "wasm32")]
mod render_web;
#[cfg(target_arch = "wasm32")]
pub use render_web::{build_canvas, register, register_from_chunk};

// WebGPU texture-layer compositor: composites a camera `MediaStream` into the
// canvas on web (via `copy_external_image_to_texture`), so a layered canvas can
// stay on the WebGPU/vello path instead of falling back to Canvas2D.
#[cfg(target_arch = "wasm32")]
mod web_layer;

// Zero-copy capture target (`render.rs` uses `NativeCapture` uniformly): the
// real IOSurface ring on macOS, a no-op stub on the other vello targets. iOS uses
// the stub for now (Stage 1 — vello renders); Stage 2 widens the real ring to iOS.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod native_capture;
// Linux (GTK4/wgpu-GL): the real zero-copy path is a dma-buf export ring — the
// analog of the macOS IOSurface ring — so Linux gets its own module, not the stub.
#[cfg(target_os = "linux")]
#[path = "native_capture_linux.rs"]
mod native_capture;
// Other non-macOS, non-Linux native targets (Windows) have no zero-copy self-capture
// path yet — the no-op stub keeps `render.rs` cfg-free and records via CPU read-back.
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "macos"), not(target_os = "linux")))]
#[path = "native_capture_stub.rs"]
mod native_capture;
