//! macOS launcher. Builds via `build-macos`, then either spawns the
//! binary as a foreground child (one-shot `idealyst run macos`) or
//! fire-and-forgets it (dev mode, where blocking on the app's
//! lifecycle would tie up the orchestrator's other targets).

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use build_ios::FrameworkSource;

#[derive(Clone, Debug)]
pub struct RunOptions {
    /// Compile with `--release`. Default: debug.
    pub release: bool,
    /// Framework-source resolution for the wrapper crate's deps.
    pub source: FrameworkSource,
    /// If true, spawn the binary detached (stdio nulled, parent
    /// returns immediately). Used by `idealyst dev` so the macOS
    /// app's lifetime is decoupled from the CLI's. One-shot
    /// `idealyst run macos` leaves this false — the user there
    /// expects a foreground process they can Ctrl-C.
    pub background: bool,
    /// Cargo features to enable on the build. `idealyst dev` passes
    /// `framework-core/dev` here so the Robot bridge auto-starts.
    pub user_features: Vec<String>,
}

#[derive(Debug)]
pub struct RunArtifact {
    /// Path to the binary that was launched.
    pub binary: PathBuf,
}

/// Build (or rebuild) the macOS wrapper for `project_dir` and launch
/// it. Foreground mode blocks until the app exits; background mode
/// returns once the binary has been spawned.
pub fn run(project_dir: &Path, opts: RunOptions) -> Result<RunArtifact> {
    let built = build_macos::build(
        project_dir,
        build_macos::BuildOptions {
            release: opts.release,
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

    if opts.background {
        // Detach: null stdio so the app doesn't fight the dev
        // orchestrator's terminal output, and leave the child
        // unwaited so we can return. If the user closes the
        // terminal the app will receive SIGHUP — that's fine here;
        // dev sessions are tied to the terminal anyway. A full
        // daemonisation (setsid) would survive close but isn't
        // what's wanted: the user wants the dev session and the
        // app to die together at the end.
        let _ = Command::new(&built.binary)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| {
                format!("spawn macOS binary {}", built.binary.display())
            })?;
    } else {
        let status = Command::new(&built.binary)
            .status()
            .with_context(|| {
                format!("spawn macOS binary {}", built.binary.display())
            })?;
        if !status.success() {
            anyhow::bail!("macOS binary exited with {status}");
        }
    }

    Ok(RunArtifact {
        binary: built.binary,
    })
}
