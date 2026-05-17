#[derive(clap::Args, Debug)]
pub struct Args {}

pub fn run(_args: Args) -> anyhow::Result<()> {
    // Will probe: rustup targets (aarch64-apple-ios, aarch64-linux-android,
    // wasm32-unknown-unknown), xcode-select, Android NDK/SDK paths,
    // wasm-pack, then print a per-platform readiness report.
    super::todo("doctor")
}
