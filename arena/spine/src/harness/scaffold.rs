//! Build the isolated project the implementation agent works in.
//!
//! Isolation has two requirements that pull against each other:
//!   * the run must test the **current** framework + MCP (not a published
//!     snapshot) → the scaffold's deps must path-point at this working tree;
//!   * the agent's only documentation source must be the MCP → its MCP config
//!     must contain exactly one server.
//!
//! `idealyst new`, given `IDEALYST_FRAMEWORK_PATH`, satisfies the first by
//! emitting `runtime-core = { path = "<repo>/crates/runtime/core" }`. We then
//! overwrite the generated `.mcp.json` to guarantee the second.
//!
//! Known, documented leak: because the deps are absolute paths into the
//! monorepo, an agent *could* `Read` the framework source instead of asking the
//! MCP. We don't sandbox the filesystem (the run needs to build); instead the
//! feedback pass flags out-of-project reads as a doc-bypass signal
//! (`metrics::doc_bypass_reads`).

use std::path::{Path, PathBuf};
use std::process::Command;

pub struct Scaffold {
    pub project_dir: PathBuf,
}

/// Scaffold a fresh idealyst app named `name` inside `dest_parent`, with its
/// framework deps path-pointed at `framework_path`, and an MCP config that
/// exposes only the idealyst server.
pub fn create(name: &str, dest_parent: &Path, framework_path: &Path) -> anyhow::Result<Scaffold> {
    std::fs::create_dir_all(dest_parent)?;
    let project_dir = dest_parent.join(name);

    let output = Command::new("idealyst")
        .arg("new")
        .arg(name)
        .current_dir(dest_parent)
        .env("IDEALYST_FRAMEWORK_PATH", framework_path)
        .output()
        .map_err(|e| anyhow::anyhow!("running `idealyst new`: {e} (is `idealyst` on PATH?)"))?;
    if !output.status.success() {
        anyhow::bail!(
            "`idealyst new {name}` failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    anyhow::ensure!(
        project_dir.join("Cargo.toml").is_file(),
        "scaffold did not produce {}/Cargo.toml",
        project_dir.display()
    );

    write_isolated_mcp_config(&project_dir)?;
    Ok(Scaffold { project_dir })
}

/// Overwrite the project's `.mcp.json` so the implementation agent sees exactly
/// one MCP server — the idealyst one — and nothing else.
pub fn write_isolated_mcp_config(project_dir: &Path) -> anyhow::Result<PathBuf> {
    let cfg = serde_json::json!({
        "mcpServers": {
            "idealyst": { "command": "idealyst", "args": ["mcp"] }
        }
    });
    let path = project_dir.join(".mcp.json");
    std::fs::write(&path, serde_json::to_string_pretty(&cfg)?)?;
    Ok(path)
}

/// Best-effort `idealyst build --web`. Returns the `dist/web` path on success,
/// `Err` with the build-error tail otherwise. Used both as the compile-tier
/// signal and as the prerequisite for the locator pass (which serves the
/// produced `dist/web/`). `robot = true` adds `--robot` so the bundle dials a
/// relay — required when the rubric has `robot`-tier items on web.
pub fn build_web(project_dir: &Path, robot: bool) -> anyhow::Result<PathBuf> {
    let mut args = vec!["build", "--web"];
    if robot {
        args.push("--robot");
    }
    let output = Command::new("idealyst")
        .args(&args)
        .current_dir(project_dir)
        .output()
        .map_err(|e| anyhow::anyhow!("running `idealyst build --web`: {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(12).collect();
        let tail: String = tail.into_iter().rev().collect::<Vec<_>>().join("\n");
        anyhow::bail!("web build failed:\n{tail}");
    }
    Ok(project_dir.join("dist").join("web"))
}
