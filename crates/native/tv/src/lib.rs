//! TV-sized native runtime variant.
//!
//! Opens a 1920 × 1080 window and reports `ColorScheme::Dark`
//! so apps that switch palettes on TV pick the right one
//! without a runtime check. The caller picks the visual skin.

use std::rc::Rc;

use framework_core::{ColorScheme, Primitive};
use host_winit::{run as run_core, DeviceProfile, RunError};
use render_wgpu::Skin;

pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;
pub const TITLE: &str = "Idealyst Preview — TV";

/// Run the TV preview with `skin`. See `native-phone` for the
/// same shape and a fuller example.
pub fn run<F>(skin: Rc<dyn Skin>, build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    run_at(skin, None, build_ui)
}

/// Same as [`run`] but places the window at a specific
/// screen-logical position.
pub fn run_at<F>(
    skin: Rc<dyn Skin>,
    position: Option<(i32, i32)>,
    build_ui: F,
) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
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
