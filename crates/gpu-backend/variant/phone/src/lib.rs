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
//! # use render_wgpu::Painter;
//! # use runtime_scene::Element;
//! # fn my_app() -> Element { todo!() }
//! # fn my_skin() -> Rc<dyn Painter> { todo!() }  // e.g. Rc::new(ios_sim::IosSim::new())
//! fn main() {
//!     variant_phone::run(my_skin(), my_app).unwrap();
//! }
//! ```

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
/// spell them `variant_phone::newcore::run` / `::run_at` / `::run_with`. There is
/// one core now and they live at the crate root — this re-export keeps
/// the historical paths resolving.
pub mod newcore {
    pub use crate::{run, run_at, run_with};
}

/// Logical width (CSS px). iPhone 14 / 15 portrait.
pub const WIDTH: u32 = 390;
/// Logical height (CSS px). iPhone 14 / 15 portrait.
pub const HEIGHT: u32 = 844;
/// Title shown in the desktop window's title bar.
pub const TITLE: &str = "Idealyst Preview — Phone";

/// Runtime-server variant of [`run`]. Instead of mounting a
/// local scene, connects to the idealyst dev-host at `url`
/// (CLI-baked via `IDEALYST_DEV_ENDPOINT`) and renders whatever
/// wire commands the sidecar streams in. Each redraw ticks the
/// runtime-server shell (which sends `RequestFrame` to drive the
/// sidecar's animation clock) AND repaints the latest scene;
/// window resizes propagate to the sidecar via the shell's
/// viewport report.
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
