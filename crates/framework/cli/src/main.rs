//! `idealyst` — framework CLI.
//!
//! Entry point for everything a developer does outside their editor:
//! scaffold new projects, run the hot-reload dev server, build and
//! deploy to each platform, sync generated assets, and check the
//! local toolchain.
//!
//! Heavy lifting lives in sibling crates — [`dev-http`](dev_http) for
//! the static-file server, [`dev-reload`](dev_reload) for the watch +
//! wasm-pack rebuild loop, [`dev-server`] for the AAS wire protocol.
//! This binary is just argument parsing and orchestration.

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
    /// Collect every `#[method]`-tagged Rust function in a project
    /// and emit them as a single BrightScript `.brs` file.
    Brs(cmd::brs::Args),
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
        Command::Brs(args) => cmd::brs::run(args),
    }
}
