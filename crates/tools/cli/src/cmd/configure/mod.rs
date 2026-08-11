//! `idealyst configure` — project configuration commands.
//!
//! A home for one-off "set up this aspect of the project" actions. Its first
//! (and today only) subcommand is [`devcontainer`], which provisions a
//! devcontainer and toggles idealyst-managed sidecar services. The command is
//! deliberately a subcommand *group* so future peers (`vscode`, …) slot in as
//! new `Sub` variants without reshaping the CLI.
//!
//! The real logic lives in the shared `configure` crate, which the MCP server
//! calls too; everything here is argument parsing + the interactive wizard.

pub mod devcontainer;
pub mod vscode;
mod wizard;
mod wizard_vscode;

use anyhow::Result;

#[derive(clap::Args, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Sub,
}

#[derive(clap::Subcommand, Debug)]
pub enum Sub {
    /// Initialize or update a devcontainer, toggling optional add-ons:
    /// sidecar services (database / redis / minio / playwright) alongside the
    /// main dev container, and AI coding agents (claude / codex) installed
    /// inside it.
    Devcontainer(devcontainer::Args),
    /// Configure VS Code workspace settings + extension recommendations: wire
    /// the idealyst linter into rust-analyzer and recommend the editor
    /// extensions an idealyst project wants.
    Vscode(vscode::Args),
}

pub fn run(args: Args) -> Result<()> {
    match args.command {
        Sub::Devcontainer(a) => devcontainer::run(a),
        Sub::Vscode(a) => vscode::run(a),
    }
}
