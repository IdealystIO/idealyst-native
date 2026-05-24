#[derive(clap::Args, Debug)]
pub struct Args {
    /// What to regenerate. Empty means "all".
    #[arg(value_enum, num_args = 0..)]
    pub kinds: Vec<SyncKind>,
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
pub enum SyncKind {
    Icons,
    Splash,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    super::todo("sync")
}
