//! Non-macOS stub. Mirrors the public surface so consumer crates
//! (the generated wrapper binary) compile on any host even though
//! the run loop is macOS-only.

use runtime_core::Element;

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

pub fn run<F: FnOnce() -> Element>(_app: F, _opts: RunOptions) -> Result<(), RunError> {
    Err(RunError::NotMacos)
}

/// Cross-host stub for [`crate::run_with`]. The `R` extension callback
/// is never invoked since the run loop can't boot off-macOS; takes the
/// same shape as the real impl so consumer code type-checks
/// uniformly. The bound is intentionally generic over `R` rather than
/// fixing a concrete `&mut MacosBackend` parameter — MacosBackend
/// isn't even compiled here.
pub fn run_with<F, R>(_app: F, _opts: RunOptions, _register_extensions: R) -> Result<(), RunError>
where
    F: FnOnce() -> Element,
{
    Err(RunError::NotMacos)
}

#[cfg(feature = "runtime-server")]
pub fn run_aas(_app_id: &str, _opts: RunOptions) -> Result<(), RunError> {
    Err(RunError::NotMacos)
}
