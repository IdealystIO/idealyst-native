//! TV-sized native runtime variant.
//!
//! Opens a 1920 × 1080 window and reports `ColorScheme::Dark`
//! so apps that switch palettes on TV pick the right one
//! without a runtime check. The caller picks the visual skin.

use std::rc::Rc;

use runtime_core::{ColorScheme, Element};
use host_winit::{run as run_core, DeviceProfile, RunError};
use render_wgpu::Painter;

pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
pub const TITLE: &str = "Idealyst Preview — TV";

/// Run the TV preview with `skin`. See `variant-phone` for the
/// same shape and a fuller example.
pub fn run<F>(skin: Rc<dyn Painter>, build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Element + 'static,
{
    run_at(skin, None, build_ui)
}

/// Same as [`run`] but places the window at a specific
/// screen-logical position.
pub fn run_at<F>(
    skin: Rc<dyn Painter>,
    position: Option<(i32, i32)>,
    build_ui: F,
) -> Result<(), RunError>
where
    F: FnOnce() -> Element + 'static,
{
    run_core(
        DeviceProfile {
            logical_size: (WIDTH, HEIGHT),
            position,
            title: TITLE.to_string(),
            color_scheme: ColorScheme::Dark,
        },
        skin,
        build_ui,
    )
}

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
