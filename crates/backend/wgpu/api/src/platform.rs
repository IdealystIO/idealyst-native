//! Which mobile OS the simulator should mimic.
//!
//! Drives the rendered look of native widgets (UISwitch vs
//! Material switch, UISlider vs Material slider, etc.) and the
//! on-screen keyboard layout. The framework's primitive tree is
//! the same across platforms — what differs is how the render
//! backend paints native-feeling controls.

/// Currently only `Ios` is fully implemented; `Android` is a
/// stub that falls through to iOS styling pending a Material 3
/// pass. Add new variants here when a render backend grows
/// support for them — both sides agree on the enum so the
/// native shell can ship a profile that selects the look
/// without knowing which render backend is wired in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SimulatedPlatform {
    Ios,
    Android,
}
