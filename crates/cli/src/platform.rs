//! The set of platforms the CLI understands. Used both as a clap
//! value-enum on the command line and as a key in project config.

use std::fmt;

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, clap::ValueEnum, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Ios,
    Android,
    Web,
    /// Application-as-a-Server dev host (not a deploy target).
    Aas,
    /// Roku (experimental).
    Roku,
    /// Wgpu-backed desktop preview ("simulator"). Not a real
    /// device target — opens a winit window on the host machine
    /// and renders the user's tree through `render-wgpu` with a
    /// phone / tablet / TV skin. Distinct from a future native
    /// macOS / Windows / Linux backend, which would use OS widget
    /// toolkits rather than custom-drawn wgpu chrome.
    Sim,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Ios => "ios",
            Platform::Android => "android",
            Platform::Web => "web",
            Platform::Aas => "aas",
            Platform::Roku => "roku",
            Platform::Sim => "sim",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
