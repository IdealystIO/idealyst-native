//! CLI-side glue around `build_ios::FrameworkSource`.
//!
//! The CLI bakes the framework's git URL + rev at compile time
//! (`crates/cli/build.rs`) and is the only place that knows them.
//! When dispatching a command, we hand `FrameworkSource::detect` the
//! defaults so it can fall back to git when no workspace is found.

use std::path::Path;

use anyhow::Result;
use build_ios::{FrameworkSource, GitDefaults};

/// Defaults captured by `build.rs`. Runtime env vars
/// `IDEALYST_FRAMEWORK_GIT_URL` / `_REV` override either of them; if
/// neither override is set, the compile-time values win.
fn git_defaults() -> GitDefaults {
    let url = std::env::var("IDEALYST_FRAMEWORK_GIT_URL")
        .unwrap_or_else(|_| env!("IDEALYST_FRAMEWORK_GIT_URL_DEFAULT").to_string());
    let rev = std::env::var("IDEALYST_FRAMEWORK_GIT_REV")
        .unwrap_or_else(|_| env!("IDEALYST_FRAMEWORK_GIT_REV_DEFAULT").to_string());
    GitDefaults { url, rev }
}

/// Resolve the framework source for `project_dir`. Single entry point
/// used by every command handler that produces wrapper Cargo.tomls.
pub fn resolve(project_dir: &Path) -> Result<FrameworkSource> {
    FrameworkSource::detect(project_dir, git_defaults())
}
