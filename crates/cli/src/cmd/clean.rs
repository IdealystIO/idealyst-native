#[derive(clap::Args, Debug)]
pub struct Args {
    /// Also drop the ephemeral platform projects under
    /// `target/idealyst/`. Without this flag, `clean` only removes
    /// Cargo build output and leaves the generated Xcode/Gradle
    /// projects in place so the next `dev` doesn't pay the
    /// regeneration cost.
    #[arg(long)]
    pub deep: bool,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    super::todo("clean")
}
