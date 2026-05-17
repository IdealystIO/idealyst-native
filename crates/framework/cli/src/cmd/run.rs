use std::path::PathBuf;

use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Target platform.
    #[arg(value_enum)]
    pub platform: Platform,

    /// Project directory. Defaults to the current directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,

    /// Build in release mode. Default is debug.
    #[arg(long)]
    pub release: bool,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    match args.platform {
        Platform::Ios => {
            let artifact = run_ios::run(
                &args.dir,
                run_ios::RunOptions {
                    release: args.release,
                },
            )?;
            eprintln!();
            eprintln!("[idealyst run ios] launched");
            eprintln!("  app:       {}", artifact.app_bundle.display());
            eprintln!("  simulator: {}", artifact.simulator_udid);
            Ok(())
        }
        _ => anyhow::bail!(
            "run for {} is not implemented yet — only ios is wired today",
            args.platform,
        ),
    }
}
