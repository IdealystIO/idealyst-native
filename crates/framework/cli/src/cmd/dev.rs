use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Which client platform to launch alongside the dev server.
    /// When omitted, only the server runs and clients connect on
    /// their own.
    #[arg(long, value_enum)]
    pub platform: Option<Platform>,

    /// TCP address the dev server binds to.
    #[arg(long, default_value = "127.0.0.1:9001")]
    pub addr: String,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    // Eventually delegates to `dev-server` crate's `serve` +
    // `spawn_rebuild_loop`, with config sourced from
    // `[package.metadata.idealyst]`.
    super::todo("dev")
}
