//! wgpu desktop-preview backend (shared core).
//!
//! Renders the application tree to a winit window via wgpu, sized
//! and chromed like a phone, tablet, or TV. The variant crates
//! ([`backend-wgpu-phone`], `-tablet`, `-tv`) wrap [`run`] with
//! device-specific [`DeviceProfile`]s.
//!
//! # Architecture
//!
//! - [`WgpuBackend`] implements [`framework_core::Backend`]. Its
//!   `Node` type is a refcounted [`node::WgpuNode`] — a tagged
//!   wrapper around per-primitive state (kind, attached handler,
//!   handle to the glyphon text buffer for text nodes, …).
//! - Layout uses [`native_layout::LayoutTree`] (Taffy). Every node
//!   carries a `LayoutNode` so the backend can run flex layout
//!   against the resolved [`StyleRules`].
//! - The renderer is two pipelines:
//!   - [`pipeline::RectPipeline`] — rounded-rect quads (one draw
//!     instance per painted node) for backgrounds + borders.
//!   - [`text::TextRenderer`] — glyphon's `TextRenderer`, fed one
//!     buffer per text node.
//! - [`app::App`] is the winit `ApplicationHandler`. It owns the
//!   surface, the backend `Rc<RefCell<WgpuBackend>>`, and the
//!   compiled pipelines. On `RedrawRequested`:
//!     1. Run the framework's reactive flush (queued effects fire).
//!     2. Run a Taffy layout pass against the current window size.
//!     3. Walk the node tree, accumulating rect + text draw commands.
//!     4. Submit to the wgpu queue.
//!
//! # MVP primitives
//!
//! The Backend trait is intentionally large; we implement the small
//! set needed to render real apps and lean on the trait's
//! `unimplemented!()` / no-op defaults for the rest. Currently:
//!
//! | Primitive       | Status                                          |
//! |-----------------|-------------------------------------------------|
//! | View            | Rounded rect (background, border, opacity)      |
//! | Text            | Glyphon, with `font_size` / `color` / wrap      |
//! | Button          | Rect + label                                    |
//! | Pressable       | Rect + click handler                            |
//! | Stack (implicit)| Comes from `insert` + Taffy flex                |
//! | apply_style     | Background, border, padding/margin, sizing      |
//! | TextInput/Toggle/Slider/Video/WebView/Image/Icon | TODO          |
//! | Virtualizer/Graphics                            | TODO          |
//!
//! Stubs return `unimplemented!()` from the trait defaults; the app
//! still renders everything else around an unsupported node.

#![allow(clippy::new_without_default)]

mod animation;
mod app;
mod backend_impl;
mod gpu;
mod host;
pub mod input;
mod node;
mod pipeline;
mod scheduler;
mod style_convert;
mod text;
mod widgets;

pub use host::Host;

pub use app::{run, DeviceProfile, RunError, SimulatedPlatform};
pub use backend_impl::WgpuBackend;
pub use node::WgpuNode;
