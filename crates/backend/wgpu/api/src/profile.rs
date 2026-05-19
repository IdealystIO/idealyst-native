//! Device-frame configuration that the variant crates pass
//! into the chosen native shell.
//!
//! Holds the cross-cutting shape information: how big the
//! window should be in logical CSS px, what to title it, what
//! color scheme to report to the app on init, and which
//! [`SimulatedPlatform`] flavor the render backend should
//! emulate.

use framework_core::ColorScheme;

use crate::platform::SimulatedPlatform;

/// What the variant tells the native shell to build.
#[derive(Clone, Debug)]
pub struct DeviceProfile {
    /// Logical width × height in CSS px. The window opens at
    /// this size; layout + hit-test happen in this logical
    /// space.
    pub logical_size: (u32, u32),
    /// Window title (shown in the title bar / dock for desktop
    /// shells; ignored by full-screen shells like browsers in
    /// iframe).
    pub title: String,
    /// Initial color scheme reported to the app on init.
    pub color_scheme: ColorScheme,
    /// OS skin the simulator should mimic. Switches widget
    /// look, keyboard layout, etc.
    pub platform: SimulatedPlatform,
}
