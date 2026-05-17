use crate::Platform;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Name of the new project. Becomes the directory name and the
    /// default `[package] name` in the generated Cargo.toml.
    pub name: String,

    /// Materialize the listed platform projects into the repo on
    /// creation. Omit to start with a config-only project that the
    /// CLI builds ephemerally — call `idealyst scaffold` later to
    /// promote a platform to a hand-editable project.
    #[arg(long, value_enum, num_args = 0.., value_delimiter = ',')]
    pub template: Vec<Platform>,

    /// Reverse-DNS bundle/application identifier (e.g.
    /// `com.example.hello`). If omitted, derived from `name`.
    #[arg(long)]
    pub bundle_id: Option<String>,
}

pub fn run(_args: Args) -> anyhow::Result<()> {
    super::todo("new")
}
