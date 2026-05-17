//! `idealyst` — framework CLI.
//!
//! Entry point for everything a developer does outside their editor:
//! scaffold new projects, run the hot-reload dev server, build and
//! deploy to each platform, sync generated assets, and check the
//! local toolchain.
//!
//! Command surface lives in [`cmd`]. Project manifest parsing lives
//! in [`config`]. Each subcommand is its own module under `cmd/` to
//! keep the dispatch in this file readable as the CLI grows.

use clap::Parser;

mod cmd;
mod config;
mod platform;

pub use platform::Platform;

#[derive(Parser, Debug)]
#[command(
    name = "idealyst",
    version,
    about = "Idealyst framework CLI",
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Scaffold a new idealyst project in a new directory.
    New(cmd::new::Args),
    /// Initialize idealyst in an existing directory.
    Init(cmd::init::Args),
    /// Start the hot-reload dev server.
    Dev(cmd::dev::Args),
    /// Produce a release build for a platform.
    Build(cmd::build::Args),
    /// Build and launch on a simulator or device.
    Run(cmd::run::Args),
    /// Run `cargo check` across configured platforms.
    Check(cmd::check::Args),
    /// Remove build artifacts.
    Clean(cmd::clean::Args),
    /// Diagnose the local toolchain (rustup targets, Xcode, NDK, …).
    Doctor(cmd::doctor::Args),
    /// Regenerate icons / splash / other derived assets from config.
    Sync(cmd::sync::Args),
    /// Materialize a platform project from the ephemeral build cache
    /// into the repo, so it can be edited by hand.
    Scaffold(cmd::scaffold::Args),
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::New(args) => cmd::new::run(args),
        Command::Init(args) => cmd::init::run(args),
        Command::Dev(args) => cmd::dev::run(args),
        Command::Build(args) => cmd::build::run(args),
        Command::Run(args) => cmd::run::run(args),
        Command::Check(args) => cmd::check::run(args),
        Command::Clean(args) => cmd::clean::run(args),
        Command::Doctor(args) => cmd::doctor::run(args),
        Command::Sync(args) => cmd::sync::run(args),
        Command::Scaffold(args) => cmd::scaffold::run(args),
    }
}
