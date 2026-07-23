//! Windows launcher. Builds via `build-windows`, then spawns the
//! `.exe` — foreground for one-shot `idealyst run windows` (blocks
//! until the window closes), detached for `idealyst dev`.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};
use build_ios::FrameworkSource;

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Compile with `--release`. Default: debug.
    pub release: bool,
    /// Framework-source resolution for the wrapper crate's deps.
    pub source: FrameworkSource,
    /// Spawn detached (stdin nulled) so the dev orchestrator's other
    /// targets can launch. One-shot `idealyst run windows` leaves this
    /// false — a foreground process the user can Ctrl-C / close.
    pub background: bool,
    /// Cargo features to enable on the build (`idealyst dev` passes
    /// `runtime-core/dev`).
    pub user_features: Vec<String>,
    /// Environment variables to set on the spawned binary (e.g. the
    /// dev Robot-bridge port-file path).
    pub env_vars: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct RunArtifact {
    /// Path to the launched `.exe`.
    pub binary: PathBuf,
    /// `Some` in background mode — the still-running child so the dev
    /// orchestrator's Ctrl-C handler can reach it. Foreground mode
    /// waits-and-drops, leaving `None`.
    pub child: Option<Child>,
}

/// Build (or rebuild) the Windows wrapper for `project_dir` and launch
/// it. Foreground blocks until the app exits; background returns once
/// spawned.
pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let built = build_windows::build(
        project_dir,
        build_windows::BuildOptions {
            release: opts.release,
            user_features: opts.user_features.clone(),
            source: opts.source,
        },
    )?;

    eprintln!(
        "[run-windows] launching {} (release={}, background={})",
        built.binary.display(),
        opts.release,
        opts.background,
    );

    let child = if opts.background {
        let mut cmd = Command::new(&built.binary);
        cmd.stdin(Stdio::null());
        for (k, v) in &opts.env_vars {
            cmd.env(k, v);
        }
        let child = cmd
            .spawn()
            .with_context(|| format!("spawn Windows binary {}", built.binary.display()))?;
        Some(child)
    } else {
        let mut cmd = Command::new(&built.binary);
        for (k, v) in &opts.env_vars {
            cmd.env(k, v);
        }
        let status = cmd
            .status()
            .with_context(|| format!("spawn Windows binary {}", built.binary.display()))?;
        if !status.success() {
            anyhow::bail!("Windows binary exited with {status}");
        }
        None
    };

    Ok(RunArtifact {
        binary: built.binary,
        child,
    })
}
