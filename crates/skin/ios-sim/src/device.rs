//! Device-model configuration for the iOS simulator skin.
//!
//! Carries the per-device knobs that affect the simulator's
//! chrome paint: notch / dynamic-island shape, viewport corner
//! radius, bezel frame, and status-bar foreground style. Bundled
//! into [`DeviceModel`] presets (e.g. iPhone 15 Pro, iPhone SE)
//! so a typical app can pick a single setting; individual knobs
//! are exposed via builder methods on the skin for fine-grained
//! overrides.
//!
//! All measurements are in logical px (matches Apple's "pt"
//! convention).

/// Camera cutout style at the top of the screen.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum NotchStyle {
    /// No cutout — original-iPhone / iPhone SE silhouette.
    None,
    /// Classic notch (iPhone X..14): an inverse-rounded
    /// rectangle hanging from the top edge. Width × height in
    /// logical px; `radius` is the corner-roundness on the
    /// notch's bottom corners.
    Notch {
        width: f32,
        height: f32,
        radius: f32,
    },
    /// Dynamic Island (iPhone 14 Pro+): a free-floating pill
    /// near the top of the display. Centered horizontally,
    /// offset down from the top by `top_offset`.
    DynamicIsland {
        width: f32,
        height: f32,
        top_offset: f32,
    },
}

/// Visible bezel framing the viewport. The simulator paints
/// this on the outermost edges over the app — the OS window's
/// frame is unchanged, the bezel is purely cosmetic. Combine
/// with a non-zero `corner_radius` to round the display
/// corners (the bezel's rounded inner corners do the visual
/// masking).
#[derive(Copy, Clone, Debug)]
pub enum BezelStyle {
    None,
    Solid {
        /// Thickness in logical px. Bezel paints inset by this
        /// amount from the viewport's outer rect.
        width: f32,
        /// sRGB color of the bezel material. iPhones run a near-
        /// black titanium / aluminum frame.
        color: [f32; 4],
    },
}

/// Foreground color theme for the status bar. Real iOS picks
/// this automatically based on the screen content under the
/// bar; the simulator keeps it as a per-skin knob since the
/// renderer doesn't sample its own framebuffer.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum StatusBarStyle {
    /// Dark glyphs (clock + status icons) on a transparent
    /// background. Best on light app surfaces.
    Dark,
    /// Light glyphs. Best on dark app surfaces / hero images.
    Light,
}

/// A bundle of chrome settings approximating a specific
/// physical device. Use [`IosSim::with_device`] to pick one;
/// chain `.with_*` builders afterward to override individual
/// knobs.
#[derive(Copy, Clone, Debug)]
pub enum DeviceModel {
    /// iPhone 15 Pro / 15 Pro Max — Dynamic Island, ~55pt
    /// corner radius, very thin titanium bezel.
    IPhone15Pro,
    /// iPhone 13 / 14 — classic notch, ~48pt corner radius,
    /// 8pt aluminum bezel.
    IPhone13,
    /// iPhone SE (3rd gen) — top chin + Touch ID home button,
    /// square corners, thin black bezel. We don't paint the
    /// home button (no fingerprint surface in the sim); the
    /// notch is `None` and corner_radius is 0.
    IPhoneSE,
}

/// Resolved per-knob settings. `IosSim` holds one of these and
/// the paint methods read each field directly. The struct is
/// `Copy` so cheap to clone for the renderer's borrow.
#[derive(Copy, Clone, Debug)]
pub struct DeviceConfig {
    pub notch: NotchStyle,
    pub corner_radius: f32,
    pub bezel: BezelStyle,
    pub status_bar_style: StatusBarStyle,
}

impl DeviceConfig {
    /// Default — same as [`DeviceModel::IPhone15Pro`].
    pub fn default_config() -> Self {
        Self::for_model(DeviceModel::IPhone15Pro)
    }

    /// Resolve a `DeviceModel` to a concrete `DeviceConfig`.
    /// Each preset bundles a notch shape, corner radius, bezel,
    /// and status-bar style that match the physical device.
    pub fn for_model(model: DeviceModel) -> Self {
        match model {
            DeviceModel::IPhone15Pro => Self {
                notch: NotchStyle::DynamicIsland {
                    width: 125.0,
                    height: 37.0,
                    top_offset: 11.0,
                },
                corner_radius: 55.0,
                bezel: BezelStyle::Solid { width: 4.0, color: BEZEL_TITANIUM },
                status_bar_style: StatusBarStyle::Dark,
            },
            DeviceModel::IPhone13 => Self {
                notch: NotchStyle::Notch {
                    width: 209.0,
                    height: 30.0,
                    radius: 20.0,
                },
                corner_radius: 48.0,
                bezel: BezelStyle::Solid { width: 6.0, color: BEZEL_BLACK },
                status_bar_style: StatusBarStyle::Dark,
            },
            DeviceModel::IPhoneSE => Self {
                notch: NotchStyle::None,
                corner_radius: 0.0,
                bezel: BezelStyle::Solid { width: 6.0, color: BEZEL_BLACK },
                status_bar_style: StatusBarStyle::Dark,
            },
        }
    }
}

/// iPhone titanium frame (Pro line). A very dark warm gray —
/// not pure black — matches the natural finish.
pub const BEZEL_TITANIUM: [f32; 4] = [
    0x29 as f32 / 255.0,
    0x29 as f32 / 255.0,
    0x2D as f32 / 255.0,
    1.0,
];
/// iPhone aluminum frame (non-Pro). Closer to pure black for
/// the black-finish version most demos default to.
pub const BEZEL_BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
