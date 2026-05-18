use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// The platform project to materialize into the repository.
    /// Once materialized (`ios/`, `android/`, etc.), the user owns
    /// the project and the CLI will not regenerate it.
    #[arg(value_enum)]
    pub platform: Platform,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    super::todo("scaffold")
}
