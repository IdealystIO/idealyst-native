use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Target platform to build.
    #[arg(value_enum)]
    pub platform: Platform,

    /// Build in release mode (the platform-native release pipeline:
    /// xcodebuild Release, gradle assembleRelease, wasm-pack
    /// --release, …).
    #[arg(long)]
    pub release: bool,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    super::todo("build")
}
