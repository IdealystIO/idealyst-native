//! `canvas-vello` — GPU renderer for the `canvas` SDK.
//!
//! Renders a `canvas_core::Scene` with [`vello`] (GPU-compute 2D) onto the
//! framework's `graphics` primitive surface via `wgpu`. Selected over
//! `canvas-native` by calling [`register`] at app bootstrap (registers an
//! `Element::External` handler for `canvas_core::CanvasProps`,
//! last-registration-wins).
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
//! A single generic [`register`] covers every backend: the GPU surface is
//! obtained from `Backend::create_graphics`, so no per-platform module is
//! needed (unlike `canvas-native`).
#![allow(missing_docs)]

// Scene→vello translation, shared by the native and web renderers (no GPU or
// async — pure op-list walk). Identical output across targets (CLAUDE.md §7).
mod encode;

// Native renderer: blocking wgpu init. macOS + iOS (objc2 0.6 Metal coexists
// with the framework's 0.2 — see Cargo.toml; iOS sim/devices use host Metal,
// which has f16/compute), Android (Vulkan), and desktop Linux/Windows.
#[cfg(not(target_arch = "wasm32"))]
mod render;
#[cfg(not(target_arch = "wasm32"))]
pub use render::register;

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

// Full-frame source-over compositor: lays vello's content over the instanced
// shape backdrop in the hybrid path. Shared by the native and web renderers.
mod compose;

// Transformed-quad compositor: composites a cached layer texture under a camera
// affine (the `DrawOp::LayerCached` fast path). Shared by the native and web
// renderers. Pure wgpu + `canvas_core`, so it isn't wasm-gated.
mod compose_transform;

// Web renderer: async wgpu init over the browser's WebGPU backend, with a
// per-canvas Canvas2D fallback when WebGPU is unavailable.
#[cfg(target_arch = "wasm32")]
mod render_web;
#[cfg(target_arch = "wasm32")]
pub use render_web::register;

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
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "macos")))]
#[path = "native_capture_stub.rs"]
mod native_capture;
