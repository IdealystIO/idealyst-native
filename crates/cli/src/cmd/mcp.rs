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

use anyhow::Result;
use clap::Args as ClapArgs;

#[derive(ClapArgs, Debug)]
pub struct Args {
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
    ///   idealyst mcp --robot --from-bin target/debug/my-app
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

pub fn run(args: Args) -> Result<()> {
    if args.check {
        return run_check();
    }

    let mut opts = mcp_server::ServerOptions::new();
    if !args.no_robot {
        opts = opts.with_robot(args.bridge);
    }
    if let Some(bin) = args.from_bin {
        // The extractor is a one-shot — invoke the user's binary
        // with `--emit-catalog` and parse its stdout. The CLI does
        // NOT cargo-build first; pre-built binaries make the reload
        // fast. Wire up `cargo build` upstream if you want
        // automatic rebuilds.
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
