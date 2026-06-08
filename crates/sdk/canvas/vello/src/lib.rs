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

// Instanced analytic-shape (rounded-box SDF) fast path for PURE-shape scenes —
// the native renderer's throughput path for a `DrawOp::Shapes` grid/scatter.
// Web stays on the encoder's expand-to-fills for now (per-canvas WebGPU is the
// constraint there); the fallback is identical output, just unaccelerated.
#[cfg(not(target_arch = "wasm32"))]
mod shape_pass;

// Web renderer: async wgpu init over the browser's WebGPU backend, with a
// per-canvas Canvas2D fallback when WebGPU is unavailable.
#[cfg(target_arch = "wasm32")]
mod render_web;
#[cfg(target_arch = "wasm32")]
pub use render_web::register;

// Zero-copy capture target (`render.rs` uses `NativeCapture` uniformly): the
// real IOSurface ring on macOS, a no-op stub on the other vello targets. iOS uses
// the stub for now (Stage 1 — vello renders); Stage 2 widens the real ring to iOS.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod native_capture;
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "macos")))]
#[path = "native_capture_stub.rs"]
mod native_capture;
