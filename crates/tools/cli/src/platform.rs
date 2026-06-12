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
    RuntimeServer,
    /// Roku (experimental).
    Roku,
    /// Wgpu-backed desktop preview ("simulator"). Not a real
    /// device target — opens a winit window on the host machine
    /// and renders the user's tree through `render-wgpu` with a
    /// phone / tablet / TV skin. Distinct from the [`Platform::Macos`]
    /// native backend below, which uses AppKit widgets rather than
    /// custom-drawn wgpu chrome.
    Sim,
    /// Native macOS via `backend-macos` + `host-appkit`. Uses real
    /// AppKit widgets (NSWindow, NSToolbar, NSView, NSButton, …).
    /// See `docs/macos-backend-plan.md`.
    Macos,
    /// TTY target via `backend-terminal` + `host-terminal`. Renders
    /// the user's tree into a crossterm grid in the current
    /// terminal. Supports `--runtime-server` like the other native
    /// targets.
    Terminal,
    /// The project's own application server — the `#[server]` RPC /
    /// API backend declared as
    /// `[package.metadata.idealyst.app].server_bin`. Not a render
    /// target: `idealyst run server` builds the web bundle and runs
    /// that binary so it serves both `/_srv/*` and the static assets.
    Server,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Ios => "ios",
            Platform::Android => "android",
            Platform::Web => "web",
            Platform::RuntimeServer => "runtime-server",
            Platform::Roku => "roku",
            Platform::Sim => "sim",
            Platform::Macos => "macos",
            Platform::Terminal => "terminal",
            Platform::Server => "server",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
