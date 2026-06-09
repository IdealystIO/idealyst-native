//! `idealyst` — framework CLI.
//!
//! Entry point for everything a developer does outside their editor:
//! scaffold new projects, run the hot-reload dev server, build and
//! deploy to each platform, sync generated assets, and check the
//! local toolchain.
//!
//! Heavy lifting lives in sibling crates — [`dev-http`](dev_http) for
//! the static-file server, [`dev-reload`](dev_reload) for the watch +
//! wasm-pack rebuild loop, [`dev-server`] for the runtime-server wire protocol.
//! This binary is just argument parsing and orchestration.

use clap::Parser;

mod cmd;
mod config;
mod dev_config;
mod dev_log;
mod framework_source;
mod memory_limit;
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
    /// Create a new project or library in a new directory.
    New(cmd::new::Args),
    /// Create a new project or library in the current directory.
    Init(cmd::init::Args),
    /// Build and run with hot reload.
    Dev(cmd::dev::Args),
    /// Build and serve a catalog-driven documentation site for a project
    /// (every `#[component]`, primitive, utility, type, guide, and icon
    /// set in the project and its component-library dependencies).
    Docs(cmd::docs::Args),
    /// Serve a directory over HTTP.
    Serve(cmd::serve::Args),
    /// Build for one or more platforms.
    Build(cmd::build::Args),
    /// Export `#[component(external)]`s as a framework-agnostic Web
    /// Component suite (wasm custom elements + .d.ts + React/Vue wrappers)
    /// into `dist/external/`.
    Export(cmd::export::Args),
    /// Build and launch on a simulator or device.
    Run(cmd::run::Args),
    /// Build a distribution-signed app and (optionally) ship it. iOS → App
    /// Store Connect (`.ipa`). macOS → Mac App Store (`.pkg`) or Developer ID
    /// notarized `.dmg`.
    Publish(cmd::publish::Args),
    /// Type-check across configured platforms.
    Check(cmd::check::Args),
    /// Remove build artifacts.
    Clean(cmd::clean::Args),
    /// Diagnose the local toolchain (Rust, web, iOS, Android; Roku pending).
    Doctor(cmd::doctor::Args),
    /// Regenerate icons, splash, and other derived assets.
    Sync(cmd::sync::Args),
    /// Inspect generated icons (preview as platform-style mockups,
    /// list output paths, etc.).
    Icon(cmd::icon::Args),
    /// Materialize a hand-editable copy of a generated platform project.
    Scaffold(cmd::scaffold::Args),
    /// Roku: transpile `#[method]`-tagged functions to BrightScript.
    Brs(cmd::brs::Args),
    /// Launch the framework MCP catalog server (stdio). Use as the
    /// `command` for an MCP client (Claude Desktop, claude.ai/code).
    /// Robot tools are on by default — pass `--no-robot` to omit them.
    Mcp(cmd::mcp::Args),
    // Hidden — cargo invokes this when the binary is used as a
    // RUSTC_WORKSPACE_WRAPPER for the runtime-server hot-patch fat build.
    // Users never call it directly.
    #[command(hide = true)]
    RustcCapture(cmd::rustc_capture::Args),
}

fn main() -> anyhow::Result<()> {
    // When cargo invokes us via `RUSTC_WORKSPACE_WRAPPER`, our
    // argv is `<idealyst-binary> <real-rustc> <rustc-args>` — clap
    // would mis-parse the rustc path as a subcommand. Sniff the
    // env-var the hot-patch fat build sets and route directly to
    // the capture handler before clap touches argv. Ordinary
    // `idealyst <cmd>` invocations don't set this var, so this
    // bypass doesn't fire for normal CLI use.
    if std::env::var_os("IDEALYST_RUSTC_CAPTURE_DIR").is_some()
        && std::env::var_os("IDEALYST_RUSTC_WRAPPER_ACTIVE").is_some()
    {
        let argv: Vec<String> = std::env::args().skip(1).collect();
        return cmd::rustc_capture::run(cmd::rustc_capture::Args { rest: argv });
    }

    // Cap our own address space so a leak in long-running modes
    // (mcp / dev / serve) trips the allocator instead of dragging
    // the editor host down with us. Set AFTER the rustc-capture
    // short-circuit above — when cargo invokes us as a rustc
    // wrapper, this process IS rustc and needs full memory. Cap is
    // per-process, so children (`cargo`, `rustc` from build cmds)
    // get their own budget; this does not constrain compilation.
    memory_limit::apply(memory_limit::DEFAULT_LIMIT_MB);

    // Auto-load `.env` / `.env.local` from the invocation dir (walking up
    // to a parent) so credentials — App Store Connect API key, signing
    // team — resolve without a manual `source`. Precedence is
    // real-env > `.env.local` > `.env`: `dotenvy` never overrides an
    // already-set var, so loading `.env.local` FIRST lets it shadow
    // `.env`, and a real shell var set before launch beats both. Missing
    // files are not an error (the common case is no `.env` at all).
    let _ = dotenvy::from_filename(".env.local");
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();
    match cli.command {
        Command::New(args) => cmd::new::run(args),
        Command::Init(args) => cmd::init::run(args),
        Command::Dev(args) => cmd::dev::run(args),
        Command::Docs(args) => cmd::docs::run(args),
        Command::Serve(args) => cmd::serve::run(args),
        Command::Build(args) => cmd::build::run(args),
        Command::Export(args) => cmd::export::run(args),
        Command::Run(args) => cmd::run::run(args),
        Command::Publish(args) => cmd::publish::run(args),
        Command::Check(args) => cmd::check::run(args),
        Command::Clean(args) => cmd::clean::run(args),
        Command::Doctor(args) => cmd::doctor::run(args),
        Command::Sync(args) => cmd::sync::run(args),
        Command::Icon(args) => cmd::icon::run(args),
        Command::Scaffold(args) => cmd::scaffold::run(args),
        Command::Brs(args) => cmd::brs::run(args),
        Command::Mcp(args) => cmd::mcp::run(args),
        Command::RustcCapture(args) => cmd::rustc_capture::run(args),
    }
}
