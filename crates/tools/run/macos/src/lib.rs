//! macOS launcher. Builds via `build-macos`, then either spawns the
//! binary as a foreground child (one-shot `idealyst run macos`) or
//! fire-and-forgets it (dev mode, where blocking on the app's
//! lifecycle would tie up the orchestrator's other targets).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};
use build_ios::FrameworkSource;

/// Which build path to spawn. `Local` mounts the user's `app()`
/// in-process via `host_appkit::run`; `Aas` connects to a dev-server
/// via `host_appkit::run_aas` and streams the sidecar's commands.
/// runtime-server produces a wrapper that does NOT depend on the user's crate
/// (the sidecar process owns it), so changes to user code don't
/// require recompiling the wrapper — only the sidecar.
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
    /// If true, spawn the binary detached (stdio nulled, parent
    /// returns immediately). Used by `idealyst dev` so the macOS
    /// app's lifetime is decoupled from the CLI's. One-shot
    /// `idealyst run macos` leaves this false — the user there
    /// expects a foreground process they can Ctrl-C.
    pub background: bool,
    /// Cargo features to enable on the build. `idealyst dev` passes
    /// `runtime-core/dev` here so the Robot bridge auto-starts.
    pub user_features: Vec<String>,
    /// Environment variables to set on the spawned binary.
    /// `idealyst dev` uses this to plumb `IDEALYST_BRIDGE_PORT_FILE`
    /// (and optionally `IDEALYST_BRIDGE_PORT`) so the running app's
    /// Robot bridge writes its port discovery file to a project-local
    /// `.idealyst/bridge.port`.
    pub env_vars: Vec<(String, String)>,
}

#[derive(Debug)]
pub struct RunArtifact {
    /// Path to the binary that was launched.
    pub binary: PathBuf,
    /// `Some` in background mode — the still-running spawned
    /// [`Child`]. Foreground mode waits-and-drops, leaving `None`.
    /// Pre-fix the caller never got a handle to the detached binary
    /// and the dev orchestrator's Ctrl-C handler couldn't kill it,
    /// so every `idealyst dev --macos` session leaked one
    /// `nicho-portfolio-macos[-aas]` process per invocation. The
    /// caller (`cli/cmd/dev.rs::launch_macos`) now pushes this into
    /// the shared `children` Vec so the SIGINT handler reaches it.
    pub child: Option<Child>,
}

/// Build (or rebuild) the macOS wrapper for `project_dir` and launch
/// it. Foreground mode blocks until the app exits; background mode
/// returns once the binary has been spawned.
pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let build_mode = match opts.mode {
        RunMode::Local => build_macos::BuildMode::Local,
        RunMode::RuntimeServer => build_macos::BuildMode::RuntimeServer,
    };
    let built = build_macos::build(
        project_dir,
        build_macos::BuildOptions {
            release: opts.release,
            mode: build_mode,
            source: opts.source,
            user_features: opts.user_features.clone(),
        },
    )?;

    eprintln!(
        "[run-macos] launching {} (release={}, background={})",
        built.binary.display(),
        opts.release,
        opts.background,
    );

    let child = if opts.background {
        // Detach: null stdin so the app doesn't fight the dev
        // orchestrator's terminal input, but pipe stdout/stderr
        // through to the orchestrator so runtime-server-mode connection logs +
        // any apply-time panic from the macOS binary actually
        // surface (pre-fix both were `Stdio::null()`, which made
        // "nothing renders" debugging impossible — the binary
        // would crash or log silently). Leave the child unwaited
        // so we can return; the returned `Child` handle goes into
        // the dev orchestrator's `children` Vec so Ctrl-C reaches
        // it.
        let mut cmd = Command::new(&built.binary);
        cmd.stdin(Stdio::null());
        for (k, v) in &opts.env_vars {
            cmd.env(k, v);
        }
        let child = cmd
            .spawn()
            .with_context(|| {
                format!("spawn macOS binary {}", built.binary.display())
            })?;
        Some(child)
    } else {
        let mut cmd = Command::new(&built.binary);
        for (k, v) in &opts.env_vars {
            cmd.env(k, v);
        }
        let status = cmd
            .status()
            .with_context(|| {
                format!("spawn macOS binary {}", built.binary.display())
            })?;
        if !status.success() {
            anyhow::bail!("macOS binary exited with {status}");
        }
        None
    };

    Ok(RunArtifact {
        binary: built.binary,
        child,
    })
}
