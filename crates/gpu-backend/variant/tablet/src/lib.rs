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

/// Re-exports [`run`] / [`run_at`] / [`run_with`] under the
/// `variant_tablet::newcore::` path older callers and docs use.
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
