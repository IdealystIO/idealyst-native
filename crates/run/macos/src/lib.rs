//! macOS launcher. Builds via `build-macos`, spawns the binary as a
//! foreground child, returns when the user quits the app.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use build_ios::FrameworkSource;

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Compile with `--release`. Default: debug.
    pub release: bool,
    /// Framework-source resolution for the wrapper crate's deps.
    pub source: FrameworkSource,
}

#[derive(Debug)]
pub struct RunArtifact {
    /// Path to the binary that was launched.
    pub binary: PathBuf,
}

/// Build (or rebuild) the macOS wrapper for `project_dir` and run
/// it as a foreground child. Returns when the child exits.
pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let built = build_macos::build(
        project_dir,
        build_macos::BuildOptions {
            release: opts.release,
            source: opts.source,
        },
    )?;

    eprintln!(
        "[run-macos] launching {} (release={})",
        built.binary.display(),
        opts.release,
    );

    let status = Command::new(&built.binary)
        .status()
        .with_context(|| format!("spawn macOS binary {}", built.binary.display()))?;
    if !status.success() {
        anyhow::bail!("macOS binary exited with {status}");
    }

    Ok(RunArtifact {
        binary: built.binary,
    })
}
