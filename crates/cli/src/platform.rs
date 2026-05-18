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
    /// Application-as-a-Server. Not a deploy target like the others —
    /// produces a dev-host binary that drives the user's reactive
    /// runtime on a developer's machine and streams primitive commands
    /// to thin clients (browser, phone) over a WebSocket. Lives in the
    /// `Platform` enum because the CLI surface treats it the same way:
    /// `idealyst build aas`, `idealyst dev --mode aas`, etc.
    Aas,
    /// Roku. Build output is a directory tree shaped like a side-loadable
    /// Roku channel package: manifest + source/*.brs + components/*.{xml,brs}
    /// + data/ui.json. `idealyst build roku` runs the build-roku pipeline,
    /// which collects `#[method]`-transpiled BrightScript and the
    /// user-produced `dist/ui.json` UI snapshot into a complete .pkg layout.
    /// Experimental — see `backend-roku`'s docs for the constraints.
    Roku,
}

impl Platform {
    pub fn as_str(self) -> &'static str {
        match self {
            Platform::Ios => "ios",
            Platform::Android => "android",
            Platform::Web => "web",
            Platform::Aas => "aas",
            Platform::Roku => "roku",
        }
    }
}

impl fmt::Display for Platform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
