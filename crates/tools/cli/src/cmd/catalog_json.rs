//! `idealyst catalog-json` — print the project's full catalog JSON to
//! stdout.
//!
//! The stable machine-facing entry point for editor tooling (the VS Code
//! completion extension shells out to this), CI, and anything else that
//! wants the catalog without speaking MCP. Reuses the exact pipeline
//! `idealyst mcp` uses internally: generate the ephemeral catalog
//! wrapper crate (idempotent — no rewrite when nothing changed), then
//! `cargo run -q --bin catalog`, whose stdout IS the catalog JSON
//! (`mcp_catalog::catalog_json()` — components, props schemas,
//! primitives, macros, utilities, types, guides).
//!
//! `DIR` may be a project directory or a cargo **workspace root**: the
//! root is expanded through the same [`resolve_project_roots`] the MCP
//! server uses, so every member with `[package.metadata.idealyst]` is
//! wrapped into one catalog (each component keeps its crate in
//! `module_path`). Editor tooling that only knows the workspace folder
//! can therefore hand it over verbatim — before this it failed to parse
//! the root as a project and produced nothing, which the VS Code
//! extension surfaced as "no completion at all" in monorepos.
//!
//! First run compiles the wrapper (the project graph with the `catalog`
//! feature on) — minutes cold, seconds warm; cargo caches everything.
//! Build chatter goes to stderr; stdout stays pure JSON.
//!
//! [`resolve_project_roots`]: super::catalog_wrapper::resolve_project_roots

use std::path::PathBuf;
use std::process::Command;

use anyhow::{bail, Context, Result};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Project directory.
    #[arg(default_value = ".")]
    pub dir: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let root = std::fs::canonicalize(&args.dir)
        .with_context(|| format!("cannot resolve project dir {}", args.dir.display()))?;

    let roots = super::catalog_wrapper::resolve_project_roots(&root)
        .context("resolve the idealyst project(s) to catalog")?;
    let wrapper_dir = super::catalog_wrapper::generate_for_roots(&roots)
        .context("generate the catalog wrapper crate")?;

    // Inherit stdout so the JSON streams straight through; `-q` keeps
    // cargo's progress off stdout (diagnostics still reach stderr).
    let status = Command::new("cargo")
        .current_dir(&wrapper_dir)
        .args(["run", "-q", "--bin", "catalog"])
        .status()
        .context("run the catalog wrapper (`cargo run -q --bin catalog`)")?;
    if !status.success() {
        bail!("catalog wrapper build/run failed (see stderr above)");
    }
    Ok(())
}
