//! Non-macOS stub. Mirrors the public surface so consumer crates
//! (the generated wrapper binary) compile on any host even though
//! the run loop is macOS-only.

use framework_core::Primitive;

#[derive(Clone, Debug, Default)]
pub struct RunOptions {
    /// Initial window title.
    pub title: String,
    /// Initial window width in points.
    pub width: f64,
    /// Initial window height in points.
    pub height: f64,
}

#[derive(Debug)]
pub enum RunError {
    /// Returned on non-macOS hosts; the AppKit run loop can't boot
    /// off-platform. Cross-compile of the wrapper binary still
    /// type-checks via this stub so `cargo check` works from Linux /
    /// Windows / iOS.
    NotMacos,
}

impl std::fmt::Display for RunError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::NotMacos => write!(
                f,
                "host-appkit can only run on macOS; cross-compile of the wrapper \
                 compiled the stub. Run on macOS to launch."
            ),
        }
    }
}

impl std::error::Error for RunError {}

pub fn run<F: FnOnce() -> Primitive>(_app: F, _opts: RunOptions) -> Result<(), RunError> {
    Err(RunError::NotMacos)
}
