//! Terminal launcher. Builds via `build-terminal`, then spawns the
//! produced binary as a foreground child with stdio inherited so
//! crossterm owns the real TTY.
//!
//! No `background` flag: the terminal host puts the TTY into raw
//! mode + alternate screen and reads stdin directly. Detaching would
//! either dead-lock fighting the orchestrator for input or render
//! into a backgrounded fd that the user can't see. `idealyst dev
//! --terminal` therefore runs the terminal app as the foreground
//! process; other targets (Robot bridge, dev-host, etc.) stay
//! background.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};
use build_ios::FrameworkSource;

/// Which build path to spawn. Mirrors [`run_macos::RunMode`] —
/// `Local` mounts the user's `app()` in-process via
/// `host_terminal::run`; `RuntimeServer` connects to a dev-host via
/// `host_terminal::run_runtime_server` and streams the sidecar's
/// commands. RuntimeServer wrappers do NOT depend on the user crate
/// (the sidecar process owns it), so user-code edits only require
/// recompiling the sidecar.
#[derive(Clone, Debug)]
pub enum RunMode {
    Local,
    RuntimeServer,
}

impl RunMode {
    pub fn is_runtime_server(&self) -> bool {
        matches!(self, RunMode::RuntimeServer)
    }
}

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Compile with `--release`. Default: debug.
    pub release: bool,
    /// Selects between local-mount (default) and runtime-server-client paths.
    pub mode: RunMode,
    /// Framework-source resolution for the wrapper crate's deps.
    pub source: FrameworkSource,
    /// Cargo features to enable on the build. `idealyst dev` passes
    /// `runtime-core/dev` here so the Robot bridge auto-starts.
    pub user_features: Vec<String>,
    /// Environment variables to set on the spawned binary.
    /// `idealyst dev` plumbs `IDEALYST_BRIDGE_PORT_FILE` (and
    /// optionally `IDEALYST_BRIDGE_PORT`) so the running app's
    /// Robot bridge writes its port discovery file to a project-
    /// local `.idealyst/bridge.port`.
    pub env_vars: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct RunArtifact {
    /// Path to the binary that was launched.
    pub binary: PathBuf,
    /// `Some` when the caller asked for a non-blocking spawn (see
    /// [`run_spawn`]); `None` after a foreground [`run`] returns.
    /// Foreground mode waits-and-drops the child.
    pub child: Option<Child>,
}

/// Build (or rebuild) the terminal wrapper for `project_dir` and
/// launch it in the foreground. Blocks until the user quits the
/// terminal app (Ctrl-C, `q`, etc.).
///
/// Stdio is inherited so the binary owns the real TTY (raw mode +
/// alternate screen come from crossterm inside the host).
pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let built = build(project_dir, &opts)?;

    eprintln!(
        "[run-terminal] launching {} (release={})",
        built.binary.display(),
        opts.release,
    );

    let mut cmd = Command::new(&built.binary);
    for (k, v) in &opts.env_vars {
        cmd.env(k, v);
    }
    let status = cmd
        .status()
        .with_context(|| format!("spawn terminal binary {}", built.binary.display()))?;
    if !status.success() {
        anyhow::bail!("terminal binary exited with {status}");
    }
    Ok(RunArtifact {
        binary: built.binary,
        child: None,
    })
}

/// Build the terminal wrapper and spawn it without waiting. Used by
/// `idealyst dev --terminal` so the orchestrator's Ctrl-C handler
/// can reach the child via the shared `children` Vec.
///
/// Stdio is still inherited — the terminal app needs the real TTY.
/// That means at most ONE inherited-stdio target may run per `dev`
/// session (so the dev CLI flushes its own startup logs before
/// handing the TTY over to the child).
pub fn run_spawn(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let built = build(project_dir, &opts)?;

    eprintln!(
        "[run-terminal] spawning {} (release={})",
        built.binary.display(),
        opts.release,
    );

    let mut cmd = Command::new(&built.binary);
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    for (k, v) in &opts.env_vars {
        cmd.env(k, v);
    }
    let child = cmd
        .spawn()
        .with_context(|| format!("spawn terminal binary {}", built.binary.display()))?;
    Ok(RunArtifact {
        binary: built.binary,
        child: Some(child),
    })
}

fn build(project_dir: &Path, opts: &RunOptions) -> Result<build_terminal::BuildArtifact> {
    let build_mode = match opts.mode {
        RunMode::Local => build_terminal::BuildMode::Local,
        RunMode::RuntimeServer => build_terminal::BuildMode::RuntimeServer,
    };
    build_terminal::build(
        project_dir,
        build_terminal::BuildOptions {
            release: opts.release,
            mode: build_mode,
            source: opts.source.clone(),
            user_features: opts.user_features.clone(),
        },
    )
}
