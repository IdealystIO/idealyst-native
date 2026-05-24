//! Device-frame configuration that the variant crates pass
//! into the chosen native shell.
//!
//! Holds the cross-cutting shape information: how big the
//! window should be in logical CSS px, what to title it, and
//! what color scheme to report to the app on init. The visual
//! skin (UIKit, M3, etc.) is selected by the caller as a
//! separate `Rc<dyn Painter>`, not encoded here.

use runtime_core::ColorScheme;

/// What the variant tells the native shell to build.
#[derive(Clone, Debug)]
pub struct DeviceProfile {
    /// Logical width × height in CSS px. The window opens at
    /// this size; layout + hit-test happen in this logical
    /// space. If the window is later resized, content scales
    /// uniformly to fit (the host shell letterboxes); the
    /// logical size never changes.
    pub logical_size: (u32, u32),
    /// Optional initial top-left position of the window, in
    /// screen-logical coordinates. `None` lets the OS place
    /// the window. Used by harnesses that open multiple
    /// preview windows and want to lay them out manually.
    pub position: Option<(i32, i32)>,
    /// Window title (shown in the title bar / dock for desktop
    /// shells; ignored by full-screen shells like browsers in
    /// iframe).
    pub title: String,
    /// Initial color scheme reported to the app on init.
    pub color_scheme: ColorScheme,
}
