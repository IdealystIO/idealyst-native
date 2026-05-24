//! Phone-sized native runtime variant.
//!
//! Opens a 390 × 844 logical-px window (matching iPhone 14/15
//! portrait) and drives the user's UI through the wgpu native
//! runtime. The visual skin is supplied by the caller — pick
//! one from `ios-sim`, `android-sim`, or any other crate that
//! implements [`render_wgpu::Painter`].
//!
//! ```no_run
//! # use std::rc::Rc;
//! # use runtime_core::Primitive;
//! # fn my_app() -> Primitive { todo!() }
//! use ios_sim::IosSim;
//!
//! fn main() {
//!     variant_phone::run(Rc::new(IosSim::new()), my_app).unwrap();
//! }
//! ```

use std::rc::Rc;

use runtime_core::{ColorScheme, Primitive};
use host_winit::{run as run_core, DeviceProfile, RunError};
use render_wgpu::Painter;

/// Logical width (CSS px). iPhone 14 / 15 portrait.
pub const WIDTH: u32 = 390;
/// Logical height (CSS px). iPhone 14 / 15 portrait.
pub const HEIGHT: u32 = 844;
/// Title shown in the desktop window's title bar.
pub const TITLE: &str = "Idealyst Preview — Phone";

/// Run the phone preview with `skin` driving every widget +
/// keyboard paint call. The skin is the only platform-flavor
/// knob; the variant crate fixes the window size + title.
pub fn run<F>(skin: Rc<dyn Painter>, build_ui: F) -> Result<(), RunError>
where
    F: FnOnce() -> Primitive + 'static,
{
    run_at(skin, None, build_ui)
}

/// Same as [`run`] but places the window at a specific
/// screen-logical position. Used by harnesses that lay out
/// multiple previews side by side.
pub fn run_at<F>(
    skin: Rc<dyn Painter>,
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

/// Runtime-server variant of [`run`]. Instead of mounting a
/// local `app()`, connects to an idealyst dev-host over the
/// network and renders whatever wire commands the sidecar
/// streams in. Discovery is by `app_id` (typically the bundle
/// id) — the dev-server's mDNS TXT record advertises the same
/// value. Each redraw ticks the runtime-server shell (which
/// sends `RequestFrame` to drive the sidecar's animation
/// clock) AND repaints the latest scene; window resizes
/// propagate to the sidecar via the shell's viewport report.
#[cfg(feature = "runtime-server")]
pub fn run_runtime_server(skin: Rc<dyn Painter>, app_id: String) -> Result<(), RunError> {
    host_winit::run_runtime_server(
        DeviceProfile {
            logical_size: (WIDTH, HEIGHT),
            position: None,
            title: TITLE.to_string(),
            color_scheme: ColorScheme::Auto,
        },
        skin,
        app_id,
    )
}
