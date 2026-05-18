//! Phone preview variant.
//!
//! Renders the user's UI inside an iPhone-shaped window
//! (390 × 844 logical px @ 1×, matching iPhone 14/15 portrait).
//! Defaults to mimicking iOS; pass [`SimulatedPlatform`] to
//! [`run_for`] to switch to Android.
//!
//! ```no_run
//! # use framework_core::Primitive;
//! # fn my_app() -> Primitive { todo!() }
//! fn main() {
//!     backend_wgpu_phone::run(my_app).unwrap();
//! }
//! ```

use backend_wgpu_core::{run as run_core, DeviceProfile, RunError, SimulatedPlatform};
use framework_core::{ColorScheme, Primitive};

pub use backend_wgpu_core::SimulatedPlatform as Platform;

/// Logical width (CSS px). iPhone 14 / 15 portrait.
pub const WIDTH: u32 = 390;
/// Logical height (CSS px). iPhone 14 / 15 portrait.
pub const HEIGHT: u32 = 844;

/// Run the phone preview mimicking iOS (default).
pub fn run<F>(build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    run_for(SimulatedPlatform::Ios, build_ui)
}

/// Run the phone preview mimicking a specific platform.
pub fn run_for<F>(platform: SimulatedPlatform, build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    let title = match platform {
        SimulatedPlatform::Ios => "Idealyst Preview — iPhone",
        SimulatedPlatform::Android => "Idealyst Preview — Android Phone",
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
