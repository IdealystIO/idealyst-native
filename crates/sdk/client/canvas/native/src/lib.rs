//! `canvas-native` — the native-2D-engine renderer for the `canvas` SDK.
//!
//! Registers an [`Element::External`](runtime_core::Element) handler for
//! `canvas_core::CanvasProps` that replays the author's [`Scene`] with
//! the platform's native 2D engine. The app selects this renderer (over
//! `canvas-vello`) by calling [`register`] once at bootstrap.
//!
//! Per-target impls live in cfg-gated modules; only one compiles per
//! build. Targets with no native module fall back to a no-op `register`
//! (the framework draws its "not supported" placeholder) — use
//! `canvas-vello` for those.
//!
//! [`Scene`]: canvas_core::Scene
#![deny(missing_docs)]

// Shared glyph-outline expansion for `DrawOp::Glyphs`, used by every CPU
// backend (web / apple / android). Gated to those targets so the fallback
// build (no native 2D engine) doesn't carry an unused skrifa dependency.
#[cfg(any(
    target_arch = "wasm32",
    all(
        any(target_os = "ios", target_os = "macos", target_os = "android"),
        not(target_arch = "wasm32")
    )
))]
mod glyphs;

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;
// Reusable Canvas2D rasterizer + capture helper — `canvas-vello`'s web renderer
// calls these as its WebGPU-unavailable fallback (renders into the graphics
// primitive's own `<canvas>`, same output as this crate's standalone handler)
// and for self-capture on its GPU path (captureStream works on any canvas).
#[cfg(target_arch = "wasm32")]
pub use web::{make_2d_rasterizer, publish_capture_stream};

// Shared CoreGraphics painter for the Apple platforms (iOS + macOS).
// The Scene→CGContext op-replay is platform-identical; only context
// acquisition + the bezier/color vtable differ per backend.
#[cfg(all(any(target_os = "ios", target_os = "macos"), not(target_arch = "wasm32")))]
mod apple;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;

#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
mod macos;
#[cfg(all(target_os = "macos", not(target_arch = "wasm32")))]
pub use macos::register;

#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
mod android;
#[cfg(all(target_os = "android", not(target_arch = "wasm32")))]
pub use android::register;

#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "ios",
    target_os = "android",
    target_os = "macos"
)))]
mod fallback {
    use runtime_core::Backend;

    /// No-op `register` for targets without a native canvas module
    /// (desktop uses `canvas-vello`). Still registers the wire serde so a
    /// canvas can round-trip over the runtime-server wire to a client that
    /// *does* have a renderer.
    pub fn register<B: Backend>(_backend: &mut B) {
        canvas_core::ensure_wire_serde();
    }
}
#[cfg(not(any(
    target_arch = "wasm32",
    target_os = "ios",
    target_os = "android",
    target_os = "macos"
)))]
pub use fallback::register;
