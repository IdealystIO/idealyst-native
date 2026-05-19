//! wgpu render backend — implements `framework_core::Backend` and
//! the [`backend_wgpu_api::EventSink`] contract.
//!
//! **No winit. No browser deps.** Any native shell that translates
//! its platform events into the `backend_wgpu_api` event vocabulary
//! and provides a wgpu surface can drive this backend.
//!
//! # Architecture
//!
//! - [`backend_impl::WgpuBackend`] — `framework_core::Backend` trait
//!   impl. Builds and mutates the node tree + Taffy layout tree.
//!   Owns the animator and the shared text + font-system stores.
//! - [`Host`] — interaction state (focus, press, drag, momentum,
//!   keyboard slide) + the `EventSink` impl. The native shell
//!   talks to the render side only through this trait.
//! - [`Renderer`] — wgpu pipeline + tree walker. Render one frame
//!   into a `wgpu::TextureView`.
//! - [`animation::Animator`] — tween engine used by both widget
//!   animations (toggle thumb) and style-driven transitions
//!   (theme crossfade).
//! - [`widgets`] — iOS / Android-skinned native widgets (toggle,
//!   slider, text input). Each `paint_*` takes a
//!   [`SimulatedPlatform`] and dispatches.
//! - [`scheduler::install_redraw_hook`] — the shell installs its
//!   redraw closure here; render-side state changes call
//!   `request_redraw()` to wake it.

#![allow(clippy::new_without_default)]

mod animation;
mod backend_impl;
mod host;
mod keyboard;
mod node;
mod pipeline;
mod renderer;
mod scheduler;
mod style_convert;
mod text;
mod widgets;

// Re-export the api vocabulary so consumers of this crate
// don't have to depend on `backend-wgpu-api` separately for
// the common types.
pub use backend_wgpu_api as api;
pub use backend_wgpu_api::{
    DeviceProfile, EventSink, Key, KeyEvent, KeyModifiers, PointerButton, PointerEvent,
    PointerId, ScrollEvent, SimulatedPlatform,
};

pub use animation::{AnimProperty, Animator, TweenKey};
pub use backend_impl::WgpuBackend;
pub use host::Host;
pub use node::WgpuNode;
pub use renderer::Renderer;
pub use scheduler::{install_redraw_hook, request_redraw};
