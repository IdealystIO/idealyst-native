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

    /// Build in release mode (the platform-native release pipeline:
    /// xcodebuild Release, gradle assembleRelease, wasm-pack
    /// --release, …).
    #[arg(long)]
    pub release: bool,

    /// iOS only: build for a physical device (`aarch64-apple-ios`)
    /// rather than the host-arch simulator (the default).
    #[arg(long)]
    pub device: bool,
}

pub fn run(args: Args) -> anyhow::Result<()> {
    match args.platform {
        Platform::Ios => {
            let artifact = build_ios::build(
                &args.dir,
                build_ios::BuildOptions {
                    release: args.release,
                    device: args.device,
                },
            )?;
            eprintln!();
            eprintln!("[idealyst build ios] success");
            eprintln!("  staticlib:    {}", artifact.staticlib.display());
            eprintln!("  target:       {}", artifact.target_triple);
            eprintln!("  wrapper crate: {}", artifact.wrapper_dir.display());
            eprintln!();
            eprintln!(
                "Next: link the staticlib into your Xcode project, declare the C \
                 entry point in a bridging header, and call `ios_main(root_view)` \
                 from your view controller's `viewDidLoad`. The `scaffold ios` \
                 command (once it lands) will generate the Xcode project for you."
            );
            Ok(())
        }
        _ => anyhow::bail!(
            "build for {} is not implemented yet — only ios is wired today",
            args.platform,
        ),
    }
}
