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
    /// Build shippable artifacts for one or more platforms. Defaults
    /// to a debug profile; pass `--release` for the production
    /// pipeline (wasm-opt, xcodebuild Release, etc.).
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
    /// Internal: rustc wrapper used by AAS dylib hot-reload to
    /// capture rustc invocations during the initial build for later
    /// replay. Hidden from `--help`; users never invoke this
    /// directly. Cargo invokes it when `RUSTC_WORKSPACE_WRAPPER` is
    /// set to the idealyst binary + this subcommand.
    #[command(hide = true)]
    RustcCapture(cmd::rustc_capture::Args),
    /// Internal: fast rebuild step for AAS dylib hot-reload. Reads
    /// the rustc invocations captured during the initial build from
    /// `<mode-dir>/.rustc-args/` and replays them directly, skipping
    /// cargo's overhead and the host bin's redundant relink.
    #[command(hide = true)]
    RebuildPatch(cmd::rebuild_patch::Args),
    /// Internal: dx-style thin-link patch builder for AAS dylib
    /// hot-reload. Synthesizes a stub object with absolute-address
    /// jumps into the running host bin, then links a minimal patch
    /// dylib with no rlib inputs.
    #[command(hide = true)]
    LinkPatch(cmd::link_patch::Args),
}

fn main() -> anyhow::Result<()> {
    // When set as `RUSTC_WRAPPER` by AAS dylib mode's initial build,
    // cargo invokes us as `<idealyst-bin> <real-rustc> <rustc-args>`.
    // Bypass clap (which would try to interpret the rustc path as a
    // subcommand) and route directly to the capture handler. The env
    // var `IDEALYST_RUSTC_CAPTURE_DIR` is the discriminator —
    // ordinary `idealyst <cmd>` invocations don't set it, so this
    // path doesn't fire for normal CLI use.
    if std::env::var_os("IDEALYST_RUSTC_CAPTURE_DIR").is_some() {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        return cmd::rustc_capture::run(cmd::rustc_capture::Args { rest: argv });
    }

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
        Command::RustcCapture(args) => cmd::rustc_capture::run(args),
        Command::RebuildPatch(args) => cmd::rebuild_patch::run(args),
        Command::LinkPatch(args) => cmd::link_patch::run(args),
    }
}
