//! Device-model configuration for the Material 3 (Android)
//! simulator skin.
//!
//! Mirrors `ios-sim::device` in shape — notch/cutout style,
//! viewport corner radius, bezel frame, status-bar foreground.
//! Android phones run a wider range of physical designs than
//! iPhones, so the presets cover representative archetypes
//! rather than specific devices.
//!
//! All measurements are in logical px (matches Android's "dp"
//! convention).

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum NotchStyle {
    /// No cutout — older devices / tablets with bezels.
    None,
    /// Centered hole-punch camera (Pixel 6+, Galaxy S20+). The
    /// circle sits in the top status-bar strip.
    HolePunchCentered { diameter: f32, top_offset: f32 },
    /// Offset hole-punch (Galaxy S10 line, some OnePlus). Same
    /// circle, biased to one side.
    HolePunchLeft { diameter: f32, top_offset: f32, left_inset: f32 },
    /// Centered teardrop / "waterdrop" notch (mid-range
    /// Androids ca. 2019). Small inverted-U at the top edge.
    Teardrop { width: f32, height: f32 },
}

#[derive(Copy, Clone, Debug)]
pub enum BezelStyle {
    None,
    Solid { width: f32, color: [f32; 4] },
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum StatusBarStyle {
    Dark,
    Light,
}

/// Representative Android device archetypes. Real devices vary
/// wildly; these presets cover the common physical layouts an
/// app team would want to preview against.
#[derive(Copy, Clone, Debug)]
pub enum DeviceModel {
    /// Pixel 8 — centered hole-punch, ~40dp corners, thin
    /// black bezel. Stock-Android reference.
    Pixel8,
    /// Galaxy S-class — offset hole-punch, ~36dp corners.
    GalaxyS,
    /// Mid-range / older — teardrop notch, modest 16dp
    /// corners, slightly thicker bezel.
    Midrange,
    /// Tablet — no cutout, square-ish 12dp corners, thin
    /// bezel. Matches Pixel Tablet / generic Android tablets.
    Tablet,
}

#[derive(Copy, Clone, Debug)]
pub struct DeviceConfig {
    pub notch: NotchStyle,
    pub corner_radius: f32,
    pub bezel: BezelStyle,
    pub status_bar_style: StatusBarStyle,
}

impl DeviceConfig {
    /// Default — same as [`DeviceModel::Pixel8`].
    pub fn default_config() -> Self {
        Self::for_model(DeviceModel::Pixel8)
    }

    pub fn for_model(model: DeviceModel) -> Self {
        match model {
            DeviceModel::Pixel8 => Self {
                notch: NotchStyle::HolePunchCentered {
                    diameter: 18.0,
                    top_offset: 6.0,
                },
                corner_radius: 40.0,
                bezel: BezelStyle::Solid { width: 4.0, color: BEZEL_BLACK },
                status_bar_style: StatusBarStyle::Dark,
            },
            DeviceModel::GalaxyS => Self {
                notch: NotchStyle::HolePunchLeft {
                    diameter: 16.0,
                    top_offset: 8.0,
                    left_inset: 32.0,
                },
                corner_radius: 36.0,
                bezel: BezelStyle::Solid { width: 4.0, color: BEZEL_GRAPHITE },
                status_bar_style: StatusBarStyle::Dark,
            },
            DeviceModel::Midrange => Self {
                notch: NotchStyle::Teardrop {
                    width: 56.0,
                    height: 18.0,
                },
                corner_radius: 16.0,
                bezel: BezelStyle::Solid { width: 6.0, color: BEZEL_BLACK },
                status_bar_style: StatusBarStyle::Dark,
            },
            DeviceModel::Tablet => Self {
                notch: NotchStyle::None,
                corner_radius: 12.0,
                bezel: BezelStyle::Solid { width: 6.0, color: BEZEL_BLACK },
                status_bar_style: StatusBarStyle::Dark,
            },
        }
    }
}

pub const BEZEL_BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
pub const BEZEL_GRAPHITE: [f32; 4] = [
    0x1A as f32 / 255.0,
    0x1A as f32 / 255.0,
    0x1F as f32 / 255.0,
    1.0,
];
