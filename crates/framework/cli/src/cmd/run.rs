use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Target platform to build and launch.
    #[arg(value_enum)]
    pub platform: Platform,

    /// Device or simulator identifier. Platform-specific (UDID for
    /// iOS, ADB serial for Android, "browser" for web).
    #[arg(long)]
    pub device: Option<String>,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    super::todo("run")
}
