use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Platforms to type-check. Empty means all configured platforms.
    #[arg(long, value_enum, num_args = 0.., value_delimiter = ',')]
    pub platform: Vec<Platform>,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    super::todo("check")
}
