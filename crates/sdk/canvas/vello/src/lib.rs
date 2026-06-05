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

// vello is available on Android (Vulkan) + desktop Linux/Windows. It is
// Enabled on macOS (objc2 0.3 coexists with the framework's 0.2 — see Cargo.toml)
// + Android/desktop. NOT on iOS (stays on `canvas-native`, retest later) or web
// (WebGL-only wgpu; web has Canvas2D via `canvas-native`).
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "ios")))]
mod render;
#[cfg(all(not(target_arch = "wasm32"), not(target_os = "ios")))]
pub use render::register;

// Zero-copy capture target (`render.rs` uses `NativeCapture` uniformly): the
// real IOSurface ring on macOS, a no-op stub on the other vello targets.
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod native_capture;
#[cfg(all(
    not(target_arch = "wasm32"),
    not(target_os = "ios"),
    not(target_os = "macos")
))]
#[path = "native_capture_stub.rs"]
mod native_capture;

/// No-op `register` on targets without vello (iOS + web): those use
/// `canvas-native`. Still registers the wire serde so a canvas can round-trip
/// over the wire.
#[cfg(any(target_arch = "wasm32", target_os = "ios"))]
pub fn register<B: runtime_core::Backend>(_backend: &mut B) {
    canvas_core::ensure_wire_serde();
}
