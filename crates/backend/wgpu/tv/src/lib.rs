//! TV preview variant.
//!
//! Renders the user's UI in a 1920 × 1080 window. Reports
//! `ColorScheme::Dark` so apps that switch palettes on TV pick
//! the right one without a runtime check.

use backend_wgpu_native::{run as run_core, DeviceProfile, RunError, SimulatedPlatform};
use framework_core::{ColorScheme, Primitive};

pub use backend_wgpu_native::SimulatedPlatform as Platform;

pub const WIDTH: u32 = 1920;
pub const HEIGHT: u32 = 1080;

/// Run the TV preview mimicking tvOS (default).
pub fn run<F>(build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    run_for(SimulatedPlatform::Ios, build_ui)
}

/// Run the TV preview mimicking a specific platform.
pub fn run_for<F>(platform: SimulatedPlatform, build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    let title = match platform {
        SimulatedPlatform::Ios => "Idealyst Preview — Apple TV",
        SimulatedPlatform::Android => "Idealyst Preview — Android TV",
    };
    run_core(
        DeviceProfile {
            logical_size: (WIDTH, HEIGHT),
            title: title.to_string(),
            color_scheme: ColorScheme::Dark,
            platform,
        },
        build_ui,
    )
}
