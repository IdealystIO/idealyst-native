//! Linux (GTK4) launcher. Builds via `build-linux`, then launches the
//! produced binary.
//!
//! Unlike the terminal host, a GTK app is a normal windowed GUI
//! process — it doesn't need the controlling TTY, so it can run either
//! in the foreground ([`run`], for one-shot `idealyst run linux`) or
//! detached ([`run_spawn`], for the `dev` orchestrator, which keeps the
//! `Child` so its Ctrl-C handler can kill it).

use std::path::{Path, PathBuf};
use std::process::{Child, Command};

use anyhow::{Context, Result};
use build_ios::FrameworkSource;

/// Which build path to spawn. Only `Local` exists today (mounts the
/// user's `app()` in-process via `host_gtk::run`); the enum mirrors the
/// other native launchers so a runtime-server variant can slot in once
/// the GTK host grows one.
#[derive(Clone, Debug)]
pub enum RunMode {
    Local,
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Compile with `--release`. Default: debug.
    pub release: bool,
    /// Selects the build path. Only [`RunMode::Local`] today.
    pub mode: RunMode,
    /// Framework-source resolution for the wrapper crate's deps.
    pub source: FrameworkSource,
    /// Run detached: [`run`] blocks until the window closes; when this
    /// is `true` the launcher spawns without waiting and returns the
    /// `Child` (used by `idealyst dev --linux`).
    pub background: bool,
    /// Cargo features to enable on the build (`idealyst dev` passes the
    /// wrapper-local `dev` feature so the Robot bridge auto-starts).
    pub user_features: Vec<String>,
    /// Environment variables to set on the spawned binary (dev plumbs
    /// bridge-discovery + launcher-pid vars here).
    pub env_vars: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct RunArtifact {
    /// Path to the binary that was launched.
    pub binary: PathBuf,
    /// `Some` when `background` was set (see [`RunOptions::background`]);
    /// `None` after a foreground run returns.
    pub child: Option<Child>,
}

/// Build (or rebuild) the Linux wrapper for `project_dir` and launch
/// it. With `background = false` (default for `idealyst run linux`)
/// this blocks until the GTK window closes; with `background = true`
/// it spawns detached and returns the `Child`.
pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let built = build(project_dir, &opts)?;

    let mut cmd = Command::new(&built.binary);
    for (k, v) in &opts.env_vars {
        cmd.env(k, v);
    }

    if opts.background {
        eprintln!("[run-linux] spawning {} (detached)", built.binary.display());
        let child = cmd
            .spawn()
            .with_context(|| format!("spawn linux binary {}", built.binary.display()))?;
        Ok(RunArtifact {
            binary: built.binary,
            child: Some(child),
        })
    } else {
        eprintln!(
            "[run-linux] launching {} (release={})",
            built.binary.display(),
            opts.release,
        );
        let status = cmd
            .status()
            .with_context(|| format!("spawn linux binary {}", built.binary.display()))?;
        if !status.success() {
            anyhow::bail!("linux binary exited with {status}");
        }
        Ok(RunArtifact {
            binary: built.binary,
            child: None,
        })
    }
}

fn build(project_dir: &Path, opts: &RunOptions) -> Result<build_linux::BuildArtifact> {
    let build_mode = match opts.mode {
        RunMode::Local => build_linux::BuildMode::Local,
    };
    build_linux::build(
        project_dir,
        build_linux::BuildOptions {
            release: opts.release,
            mode: build_mode,
            source: opts.source.clone(),
            user_features: opts.user_features.clone(),
        },
    )
}
