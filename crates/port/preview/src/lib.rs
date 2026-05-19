//! Port a React/Vue/Svelte project, scaffold a compilable Rust
//! scratch crate around the output, and run `cargo check` against
//! it to verify the porter produced coherent code. Optionally
//! generates a rendering `main.rs` that wires the ported root
//! component to the wgpu desktop preview host.
//!
//! See `Cargo.toml` for the pipeline overview.

pub mod root;
pub mod scaffold;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use port_project::report::ProjectReport;

use scaffold::Workspace;

/// One preview run.
pub struct PreviewConfig<'a> {
    /// Source root — a local path or a git URL (the latter is
    /// handled by `port-project`'s existing `git::clone_to_temp`).
    pub input: &'a Path,
    /// Where to write the scratch crate.
    pub output_dir: &'a Path,
    /// Filesystem path to the idealyst-native checkout root. The
    /// scratch crate's `Cargo.toml` will reference framework-core
    /// (and, when rendering, framework-theme / render-wgpu /
    /// ios-sim / native-phone) via path dependencies under this
    /// root. Required because the scratch crate is *not* a
    /// workspace member.
    pub workspace_root: &'a Path,
    /// PascalCase component name to use as the render root.
    /// Defaults to `"App"`. If found, a rendering `main.rs` is
    /// generated; if not, a compile-check-only `main.rs` is
    /// generated instead.
    pub root_name: &'a str,
    /// Optional input-path filter. When `Some`, the root
    /// component is only searched in files matching this path
    /// suffix (e.g. `--root-file src/App.tsx`).
    pub root_file: Option<&'a Path>,
}

/// Outcome of one preview attempt.
pub struct PreviewOutcome {
    pub project_report: ProjectReport,
    pub check_result: CheckResult,
    pub scratch_dir: PathBuf,
    /// Set when port-preview located a root component and
    /// generated a rendering `main.rs`. Unset when it fell back
    /// to compile-check-only mode.
    pub root: Option<root::RootMatch>,
}

pub enum CheckResult {
    /// `cargo check` exited zero. The ported tree compiles.
    Ok,
    /// `cargo check` failed. `stderr` is captured so the caller
    /// can show it (or count errors etc.).
    Failed { stderr: String },
    /// We couldn't even run cargo (binary missing, IO error).
    DidNotRun { reason: String },
}

pub fn preview(cfg: &PreviewConfig) -> std::io::Result<PreviewOutcome> {
    fs::create_dir_all(cfg.output_dir.join("src/ported"))?;

    // Phase 1: port the project, writing `.rs` output into the
    // `src/ported/` subdirectory of the scratch crate.
    let project_report = port_project::port_project(&port_project::PortConfig {
        input_root: cfg.input,
        output_root: &cfg.output_dir.join("src/ported"),
    })?;

    // Phase 2: locate the root component (if any).
    let root_match = root::find(
        &project_report.files,
        cfg.root_name,
        cfg.root_file,
    );

    // Phase 3: scaffold. Module tree is the same either way;
    // Cargo.toml + main.rs vary by whether we found a root.
    let workspace = Workspace::from_root(cfg.workspace_root);
    let with_host = root_match.is_some();

    scaffold::write_cargo_toml(cfg.output_dir, &workspace, with_host)?;
    scaffold::write_mod_tree(&cfg.output_dir.join("src/ported"))?;

    match &root_match {
        Some(m) => {
            let module_path = root::module_path_for(&m.output_path, cfg.output_dir)
                .ok_or_else(|| {
                    std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "could not derive module path for root component",
                    )
                })?;
            scaffold::write_main_rs_rendering(
                cfg.output_dir,
                &module_path,
                &m.fn_name,
                &m.component_name,
            )?;
        }
        None => {
            scaffold::write_main_rs_check_only(cfg.output_dir)?;
        }
    }

    // Phase 4: run `cargo check` against the scratch crate.
    let check_result = run_cargo_check(cfg.output_dir);

    Ok(PreviewOutcome {
        project_report,
        check_result,
        scratch_dir: cfg.output_dir.to_path_buf(),
        root: root_match,
    })
}

fn run_cargo_check(scratch: &Path) -> CheckResult {
    let manifest = scratch.join("Cargo.toml");
    let output = Command::new("cargo")
        .args(["check", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .env("CARGO_TERM_COLOR", "never")
        .output();
    let output = match output {
        Ok(o) => o,
        Err(e) => return CheckResult::DidNotRun { reason: e.to_string() },
    };
    if output.status.success() {
        CheckResult::Ok
    } else {
        CheckResult::Failed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }
}

/// Try to locate the idealyst-native workspace root by walking up
/// from a starting directory until we find
/// `crates/framework/core/Cargo.toml`. Returns the workspace root
/// (the directory containing `crates/`), not the framework-core
/// crate directory.
pub fn discover_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current: Option<&Path> = Some(start);
    while let Some(dir) = current {
        if dir.join("crates/framework/core/Cargo.toml").is_file() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}
