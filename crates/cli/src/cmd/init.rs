use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Materialize these platform projects into the current
    /// directory. See `new --template` for the same flag.
    #[arg(long, value_enum, num_args = 0.., value_delimiter = ',')]
    pub template: Vec<Platform>,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    super::todo("init")
}
