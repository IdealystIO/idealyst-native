//! Tablet-sized native runtime variant.
//!
//! Opens an 820 × 1180 logical-px window (matching iPad 10.9"
//! portrait). The caller picks the visual skin.

// Only the runtime-server boot below needs these; the local-mount
// entries live in `boot.rs`.
#[cfg(feature = "runtime-server")]
use std::rc::Rc;
#[cfg(feature = "runtime-server")]
use host_winit::{DeviceProfile, RunError};
#[cfg(feature = "runtime-server")]
use render_wgpu::Painter;
#[cfg(feature = "runtime-server")]
use runtime_shared::ColorScheme;

mod boot;

pub use boot::{run, run_at, run_with};

/// Compatibility path. These entries used to live behind a `newcore`
/// module while the framework carried two cores; callers and docs
/// spell them `variant_tablet::newcore::run` / `::run_at` / `::run_with`. There is
/// one core now and they live at the crate root — this re-export keeps
/// the historical paths resolving.
pub mod newcore {
    pub use crate::{run, run_at, run_with};
}

pub const WIDTH: u32 = 820;
pub const HEIGHT: u32 = 1180;
pub const TITLE: &str = "Idealyst Preview — Tablet";

/// Runtime-server variant of [`run`]. See `variant_phone::run_runtime_server`
/// for the full per-frame behavior — only the window profile
/// (size + title) differs here.
#[cfg(feature = "runtime-server")]
pub fn run_runtime_server(skin: Rc<dyn Painter>, url: String) -> Result<(), RunError> {
    host_winit::run_runtime_server(
        DeviceProfile {
            logical_size: (WIDTH, HEIGHT),
            position: None,
            title: TITLE.to_string(),
            color_scheme: ColorScheme::Auto,
        },
        skin,
        url,
    )
}
