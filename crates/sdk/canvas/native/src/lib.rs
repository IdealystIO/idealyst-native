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

#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_arch = "wasm32")]
pub use web::register;

#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
mod ios;
#[cfg(all(target_os = "ios", not(target_arch = "wasm32")))]
pub use ios::register;

#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
mod fallback {
    use runtime_core::Backend;

    /// No-op `register` for targets without a native canvas module yet
    /// (Android lands next; desktop uses `canvas-vello`). Still registers
    /// the wire serde so a canvas can round-trip over the runtime-server
    /// wire to a client that *does* have a renderer.
    pub fn register<B: Backend>(_backend: &mut B) {
        canvas_core::ensure_wire_serde();
    }
}
#[cfg(not(any(target_arch = "wasm32", target_os = "ios")))]
pub use fallback::register;
