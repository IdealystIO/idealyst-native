//! `idealyst mcp` — launch the framework MCP catalog server.
//!
//! Run as a long-lived stdio process — wire it into Claude Desktop /
//! claude.ai/code / any MCP client as their `command`. The server
//! exposes:
//!
//! - The **static catalog** of `#[component]` / `#[idealyst_tool]`
//!   functions discovered via `inventory::submit!` in this binary's
//!   link image. The component graph (composes / uses), per-prop
//!   schema fields, etc.
//! - The **Robot tools** when `--robot` is on: `find_element`,
//!   `click`, `type_text`, `get_snapshot`, and so on. These proxy
//!   to the running app's Robot bridge over TCP.
//!
//! Either side degrades gracefully — the catalog is always served;
//! Robot tools return "is the app running?" when the bridge is
//! unreachable.
//!
//! ```text
//! idealyst mcp                      # catalog + Robot (Robot on by default)
//! idealyst mcp --no-robot           # catalog-only (e.g. for CI doc-gen)
//! idealyst mcp --bridge HOST:PORT   # custom bridge address
//! idealyst mcp --check              # lint pass, exit non-zero on findings
//! ```

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, Subcommand};

#[derive(ClapArgs, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Skip the Robot tools, leaving only the static catalog. Robot
    /// is on by default — it's part of the dev configuration, not
    /// an opt-in. Pass this when you specifically want a
    /// catalog-only server (CI doc-gen, etc.).
    #[arg(long)]
    pub no_robot: bool,

    /// Robot bridge address (host:port). Default 127.0.0.1:9718,
    /// matching `framework_core::robot::bridge::DEFAULT_PORT`.
    #[arg(long, default_value = mcp_server::DEFAULT_BRIDGE)]
    pub bridge: String,

    /// Lint the catalog and exit non-zero on findings instead of
    /// starting the server. Useful as a CI gate.
    #[arg(long)]
    pub check: bool,

    /// Path to a binary that, when invoked with `--emit-catalog`,
    /// prints the project's catalog JSON to stdout. The CLI's own
    /// inventory is empty (no `#[component]`s are defined here), so
    /// without this flag the catalog tools return no results.
    /// Typical usage:
    ///
    ///   idealyst mcp --from-bin target/debug/my-app
    ///
    /// The CLI spawns this binary at startup (and again on file
    /// change if `--watch` is set) and pipes the JSON back into the
    /// live catalog. The user's binary needs to support
    /// `--emit-catalog` mode (one line: print
    /// `framework_mcp::catalog_json()` and exit).
    #[arg(long)]
    pub from_bin: Option<std::path::PathBuf>,

    /// Watch source directories and refresh the catalog on change.
    /// Requires `--from-bin` to do anything useful — the watcher
    /// re-runs the extractor on every save. Pass once per dir.
    #[arg(long = "watch", value_name = "DIR")]
    pub watch_dirs: Vec<std::path::PathBuf>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Write a `.mcp.json` file at the current directory pointing at
    /// `idealyst mcp`. Claude Code auto-loads `.mcp.json` from the
    /// project root, so this is what hooks an existing project into
    /// the framework's MCP server. Idempotent — overwrites any
    /// existing `.mcp.json` with the canonical default contents.
    Install(InstallArgs),
}

#[derive(ClapArgs, Debug)]
pub struct InstallArgs {
    /// Project directory. Defaults to the current directory.
    #[arg(default_value = ".")]
    pub dir: std::path::PathBuf,

    /// Server name in `.mcp.json`. Defaults to `idealyst`. Change
    /// only if you already have another server registered under
    /// that name in the same project.
    #[arg(long, default_value = "idealyst")]
    pub name: String,

    /// Path to the `idealyst` binary the MCP client should spawn.
    /// Defaults to bare `idealyst` (resolved from `$PATH` at launch
    /// time — works after `cargo install idealyst-cli`).
    #[arg(long, default_value = "idealyst")]
    pub command: String,

    /// Overwrite an existing `.mcp.json` without prompting.
    /// Without `--force`, the command refuses if the file already
    /// exists.
    #[arg(long)]
    pub force: bool,
}

pub fn run(args: Args) -> Result<()> {
    if let Some(cmd) = args.command {
        return match cmd {
            Command::Install(install_args) => run_install(install_args),
        };
    }
    if args.check {
        return run_check();
    }

    let mut opts = mcp_server::ServerOptions::new();
    if !args.no_robot {
        // Auto-discover the bridge from `.idealyst/bridge.port` in
        // cwd (or any ancestor). When no port file is found we fall
        // back to the user-supplied (or default) `--bridge` value;
        // when the file's project_root doesn't match cwd we get
        // `None` back and the Robot tools stay disabled — a
        // safeguard so an MCP session in project A can't drive
        // project B's app.
        if let Some(addr) = mcp_server::resolve_bridge_addr(&args.bridge) {
            opts = opts.with_robot(addr);
        } else {
            eprintln!(
                "[idealyst mcp] Robot tools disabled — bridge port file points at a different project. \
                 Check `.idealyst/bridge.port` in your project root; it should reference {:?}.",
                std::env::current_dir().ok().map(|p| p.display().to_string()).unwrap_or_default(),
            );
        }
    }
    // Catalog binary: explicit `--from-bin` wins; otherwise
    // auto-discover via `.idealyst/catalog.path` (written by
    // `idealyst dev` after a successful build). The extractor is
    // invoked with `--emit-catalog` and its stdout parsed as the
    // catalog JSON.
    let catalog_bin = args.from_bin.or_else(mcp_server::resolve_catalog_bin);
    if let Some(bin) = catalog_bin {
        let bin = std::sync::Arc::new(bin);
        opts = opts.with_subprocess_catalog(move || {
            let mut c = std::process::Command::new(bin.as_path());
            c.arg("--emit-catalog");
            c
        });
    }
    if !args.watch_dirs.is_empty() {
        opts = opts.with_watch(args.watch_dirs);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("tokio runtime");

    rt.block_on(async move {
        mcp_server::run_stdio_with_full_options(opts).await
    })
    .map_err(|e| anyhow::anyhow!("mcp server exited: {:?}", e))
}

fn run_install(args: InstallArgs) -> Result<()> {
    let target = args.dir.join(".mcp.json");
    if target.exists() && !args.force {
        anyhow::bail!(
            "{} already exists. Pass --force to overwrite.",
            target.display()
        );
    }

    let body = serde_json::json!({
        "mcpServers": {
            args.name.clone(): {
                "command": args.command,
                "args": ["mcp"],
            }
        }
    });
    let pretty = serde_json::to_string_pretty(&body)? + "\n";
    std::fs::write(&target, pretty)
        .with_context(|| format!("write {}", target.display()))?;

    eprintln!("[idealyst mcp install] wrote {}", target.display());
    eprintln!(
        "[idealyst mcp install] Server name: {}, command: {}",
        args.name, args.command
    );
    eprintln!(
        "[idealyst mcp install] Run `idealyst dev` to launch the app; the bridge \
         port is written to .idealyst/bridge.port and the MCP server discovers \
         it from cwd."
    );
    Ok(())
}

fn run_check() -> Result<()> {
    let cat = framework_mcp::ResolvedCatalog::build();
    let findings = mcp_server::lint_catalog(&cat);
    if findings.is_empty() {
        println!(
            "OK — {} components, no catalog-integrity issues",
            cat.entries().len()
        );
        return Ok(());
    }
    for f in &findings {
        let tag = match f.severity {
            mcp_server::Severity::Warning => "warn",
            mcp_server::Severity::Error => "error",
        };
        println!("[{}] {} — {}", tag, f.fqn, f.message);
    }
    println!("\n{} findings", findings.len());
    std::process::exit(1);
}
