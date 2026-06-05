//! `canvas-vello` — GPU renderer for the `canvas` SDK.
//!
//! Renders a `canvas_core::Scene` with [`vello`] (GPU-compute 2D) onto the
//! framework's `graphics` primitive surface via `wgpu`. Selected over
//! `canvas-native` by calling [`register`] at app bootstrap (registers an
//! `Element::External` handler for `canvas_core::CanvasProps`,
//! last-registration-wins).
//!
//! Native-only: vello needs compute shaders (Metal / Vulkan / DX12 /
//! WebGPU). Web has a native 2D API (Canvas2D via `canvas-native`) and the
//! repo's web `wgpu` is WebGL-only, so `register` is a no-op on wasm32.
//!
//! A single generic [`register`] covers every native backend: the GPU
//! surface is obtained from `Backend::create_graphics`, so no per-platform
//! module is needed (unlike `canvas-native`).
#![allow(missing_docs)]

// vello runs on every native backend: macOS + iOS (objc2 0.6 Metal coexists with
// the framework's 0.2 — see Cargo.toml; iOS sim/devices use host Metal, which has
// f16/compute), Android (Vulkan), and desktop Linux/Windows. NOT on web (WebGL-only
// wgpu; web has Canvas2D via `canvas-native`).
#[cfg(not(target_arch = "wasm32"))]
mod render;
#[cfg(not(target_arch = "wasm32"))]
pub use render::register;

// Zero-copy capture target (`render.rs` uses `NativeCapture` uniformly): the
// real IOSurface ring on macOS, a no-op stub on the other vello targets. iOS uses
// the stub for now (Stage 1 — vello renders); Stage 2 widens the real ring to iOS.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod native_capture;
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "macos")))]
#[path = "native_capture_stub.rs"]
mod native_capture;

/// No-op `register` on web (WebGL-only wgpu): uses `canvas-native` (Canvas2D).
/// Still registers the wire serde so a canvas can round-trip over the wire.
#[cfg(target_arch = "wasm32")]
pub fn register<B: runtime_core::Backend>(_backend: &mut B) {
    canvas_core::ensure_wire_serde();
}
