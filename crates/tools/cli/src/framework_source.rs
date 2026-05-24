//! CLI-side glue around `build_ios::FrameworkSource`.
//!
//! The CLI bakes the framework's git URL + refspec at compile time
//! (`crates/cli/build.rs`) and is the only place that knows them.
//! When dispatching a command, we hand `FrameworkSource::detect` the
//! defaults so it can fall back to git when no workspace is found.
//!
//! Refspec is either a tag (preferred — `tag = "v0.1.0"`) or a
//! commit hash. `build.rs` picks based on whether HEAD is tagged.
//! Either form can be overridden at runtime via env vars:
//! `IDEALYST_FRAMEWORK_GIT_TAG`, `IDEALYST_FRAMEWORK_GIT_REV`, or
//! `IDEALYST_FRAMEWORK_GIT_URL`.

use std::path::Path;

use anyhow::Result;
use build_ios::{FrameworkSource, GitDefaults, GitRef};

fn git_defaults() -> GitDefaults {
    let url = std::env::var("IDEALYST_FRAMEWORK_GIT_URL")
        .unwrap_or_else(|_| env!("IDEALYST_FRAMEWORK_GIT_URL_DEFAULT").to_string());

    // Runtime overrides win — tag beats rev when both are set,
    // matching the build.rs ordering.
    let refspec = if let Ok(tag) = std::env::var("IDEALYST_FRAMEWORK_GIT_TAG") {
        if !tag.is_empty() {
            GitRef::Tag(tag)
        } else {
            compile_time_refspec()
        }
    } else if let Ok(rev) = std::env::var("IDEALYST_FRAMEWORK_GIT_REV") {
        if !rev.is_empty() {
            GitRef::Rev(rev)
        } else {
            compile_time_refspec()
        }
    } else {
        compile_time_refspec()
    };

    GitDefaults { url, refspec }
}

/// The refspec `build.rs` captured into the binary. `KIND` is the
/// cargo dep-table key (`rev`, `tag`, `branch`); `VALUE` is the
/// corresponding string.
fn compile_time_refspec() -> GitRef {
    let kind = env!("IDEALYST_FRAMEWORK_GIT_REF_KIND_DEFAULT");
    let value = env!("IDEALYST_FRAMEWORK_GIT_REF_VALUE_DEFAULT").to_string();
    match kind {
        "tag" => GitRef::Tag(value),
        "branch" => GitRef::Branch(value),
        // Default + "rev" both land here so an unknown KIND (forward-
        // compat / future variants) degrades to commit-pinning.
        _ => GitRef::Rev(value),
    }
}

/// Resolve the framework source for `project_dir`. Single entry point
/// used by every command handler that produces wrapper Cargo.tomls.
pub fn resolve(project_dir: &Path) -> Result<FrameworkSource> {
    FrameworkSource::detect(project_dir, git_defaults())
}
