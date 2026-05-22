//! `idealyst mcp` — launch the framework MCP catalog server.
//!
//! Run as a long-lived stdio process — wire it into Claude Desktop /
//! claude.ai/code / any MCP client as their `command`. The server
//! exposes:
//!
//! - The **static catalog** of `#[component]` / `#[idealyst_tool]`
//!   functions, plus framework primitives / utilities / guides.
//!   Sourced live from running apps over their Robot bridge's
//!   `get_catalog` command (discovered via mDNS), or from a project's
//!   catalog binary at startup when no app is running.
//! - The **Robot tools**: `find_element`, `click`, `type_text`,
//!   `get_snapshot`, and so on. These proxy to the running app's
//!   Robot bridge over TCP, discovered via mDNS
//!   (`_idealyst-robot._tcp.local.`).
//!
//! Either side degrades gracefully — when no app is running the
//! catalog falls back to the in-process catalog (or `--project-root`
//! extracted catalog), and Robot tools return "no app running."
//!
//! ```text
//! idealyst mcp                      # catalog + Robot
//! idealyst mcp --no-robot           # catalog-only (e.g. CI doc-gen)
//! idealyst mcp --project-root DIR   # extract catalog from DIR's catalog binary at startup
//! idealyst mcp --check              # lint pass, exit non-zero on findings
//! ```

use anyhow::{Context, Result};
use clap::{Args as ClapArgs, Subcommand};

#[derive(ClapArgs, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Option<Command>,

    /// Skip the Robot tools, leaving only the catalog. Pass this when
    /// you specifically want a catalog-only server (CI doc-gen, etc.).
    #[arg(long)]
    pub no_robot: bool,

    /// Lint the catalog and exit non-zero on findings instead of
    /// starting the server. Useful as a CI gate.
    #[arg(long)]
    pub check: bool,

    /// Path to a project directory whose catalog binary should populate
    /// the server's catalog at startup. The CLI looks for
    /// `<dir>/target/debug/catalog` (then `target/release/catalog`),
    /// invokes it with `--emit-catalog`, and pipes the JSON into the
    /// live catalog. Use this when running the MCP server against a
    /// project that isn't currently running — when an app IS running,
    /// the catalog flows automatically over its Robot bridge.
    /// Defaults to cwd when no apps are live and no explicit value
    /// is given.
    #[arg(long, value_name = "DIR")]
    pub project_root: Option<std::path::PathBuf>,

    /// Path to an explicit catalog binary. Bypasses the
    /// `--project-root` lookup. Same emit contract: invoked with
    /// `--emit-catalog`, stdout parsed as the catalog JSON.
    #[arg(long)]
    pub from_bin: Option<std::path::PathBuf>,

    /// Watch source directories and refresh the catalog on change.
    /// Requires `--from-bin` or `--project-root` to be useful — the
    /// watcher re-runs the extractor on every save. Pass once per dir.
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
    // Robot routing is mDNS-only now — no explicit `--bridge` flag.
    // The CatalogService's discovery thread maintains a live table
    // of `_idealyst-robot._tcp` advertisements; the resolver picks
    // the unique live app, or by `app` arg when multiple are
    // running. Off when `--no-robot` is set.
    if !args.no_robot {
        opts = opts.with_robot_mdns();
    }

    // Catalog binary resolution:
    // 1. Explicit `--from-bin` wins.
    // 2. `--project-root <dir>` looks for `<dir>/target/{debug,release}/catalog`.
    // 3. Neither flag: skip — the catalog is whatever the mDNS-discovered
    //    apps surface live, falling back to the in-process catalog.
    let catalog_bin = args.from_bin.or_else(|| {
        let root = args.project_root.as_ref()?;
        find_catalog_binary(root)
    });
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

/// Resolve a project's catalog binary by looking at the scaffolded
/// `[[bin]] name = "catalog"` output path. Prefers `target/debug` over
/// `target/release` since dev is the typical flow; release is checked
/// as a fallback in case the user pre-built for production.
fn find_catalog_binary(project_root: &std::path::Path) -> Option<std::path::PathBuf> {
    for profile in ["debug", "release"] {
        let candidate = project_root.join("target").join(profile).join("catalog");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
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
