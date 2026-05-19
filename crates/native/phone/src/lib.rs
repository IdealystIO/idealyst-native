//! Phone-sized native runtime variant.
//!
//! Opens a 390 × 844 logical-px window (matching iPhone 14/15
//! portrait) and drives the user's UI through the wgpu native
//! runtime. The visual skin is supplied by the caller — pick
//! one from `ios-sim`, `android-sim`, or any other crate that
//! implements [`render_wgpu::Skin`].
//!
//! ```no_run
//! # use std::rc::Rc;
//! # use framework_core::Primitive;
//! # fn my_app() -> Primitive { todo!() }
//! use ios_sim::IosSim;
//!
//! fn main() {
//!     native_phone::run(Rc::new(IosSim::new()), my_app).unwrap();
//! }
//! ```

use std::rc::Rc;

use framework_core::{ColorScheme, Primitive};
use host_winit::{run as run_core, DeviceProfile, RunError};
use render_wgpu::Skin;

/// Logical width (CSS px). iPhone 14 / 15 portrait.
pub const WIDTH: u32 = 390;
/// Logical height (CSS px). iPhone 14 / 15 portrait.
pub const HEIGHT: u32 = 844;
/// Title shown in the desktop window's title bar.
pub const TITLE: &str = "Idealyst Preview — Phone";

/// Run the phone preview with `skin` driving every widget +
/// keyboard paint call. The skin is the only platform-flavor
/// knob; the variant crate fixes the window size + title.
pub fn run<F>(skin: Rc<dyn Skin>, build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    run_at(skin, None, build_ui)
}

/// Same as [`run`] but places the window at a specific
/// screen-logical position. Used by harnesses that lay out
/// multiple previews side by side.
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
            color_scheme: ColorScheme::Auto,
        },
        skin,
        build_ui,
    )
}
