//! Tablet preview variant.
//!
//! Renders the user's UI inside an iPad-shaped window
//! (820 × 1180 logical px @ 1×, matching iPad 10.9" portrait).

use backend_wgpu_core::{run as run_core, DeviceProfile, RunError, SimulatedPlatform};
use framework_core::{ColorScheme, Primitive};

pub use backend_wgpu_core::SimulatedPlatform as Platform;

pub const WIDTH: u32 = 820;
pub const HEIGHT: u32 = 1180;

/// Run the tablet preview mimicking iOS (default).
pub fn run<F>(build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    run_for(SimulatedPlatform::Ios, build_ui)
}

/// Run the tablet preview mimicking a specific platform.
pub fn run_for<F>(platform: SimulatedPlatform, build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    let title = match platform {
        SimulatedPlatform::Ios => "Idealyst Preview — iPad",
        SimulatedPlatform::Android => "Idealyst Preview — Android Tablet",
    };
    run_core(
        DeviceProfile {
            logical_size: (WIDTH, HEIGHT),
            title: title.to_string(),
            color_scheme: ColorScheme::Auto,
            platform,
        },
        build_ui,
    )
}
