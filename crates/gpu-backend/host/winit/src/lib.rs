//! Winit native shell for the wgpu render backend.
//!
//! Translates `winit::event::WindowEvent` → the
//! [`render_api`] event vocabulary, owns the wgpu surface
//! (built from a winit `Window`), and drives
//! `render_wgpu::Renderer` per frame.
//!
//! Variant crates (`variant-phone`, `-tablet`, `-tv`) call [`run`] /
//! [`run_with`] with a [`DeviceProfile`].

mod app;
mod gpu;
mod scheduler;

pub use app::{run, run_with, RunError};

/// Compatibility path. The windowed boot used to live behind a
/// `newcore` module while the framework carried two cores; callers and
/// docs spell it `host_winit::newcore::run` / `::run_with`. There is
/// one core now and the entries live at the crate root ([`crate::run`],
/// [`crate::run_with`]) — this re-export keeps the historical paths
/// resolving.
pub mod newcore {
    pub use crate::{run, run_with};
}

#[cfg(feature = "runtime-server")]
pub use app::run_runtime_server;

// The variant + user code consumes these via this crate so they
// don't need a direct dependency on `render-api`.
pub use render_api::DeviceProfile;
