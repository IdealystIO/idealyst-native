//! Bake compile-time git defaults into the `idealyst` binary.
//!
//! The CLI scaffolds wrapper Cargo.tomls that depend on the framework
//! crates. When the project lives outside the framework workspace
//! (the `cargo install idealyst-cli` case), those deps need a git
//! URL + commit. We capture both at *CLI* compile time so a given
//! installed binary always scaffolds projects pinned to the framework
//! commit it was built against — predictable, reproducible, no
//! "default branch moved underneath me" surprises.
//!
//! Both values are overridable at build time via env vars so forks /
//! private mirrors / CI builds can repoint without source edits, and
//! again at *runtime* via the same env vars on the resulting binary.

use std::process::Command;

/// Public framework repo. Override at build time with
/// `IDEALYST_FRAMEWORK_GIT_URL=...` for forks or private mirrors.
const DEFAULT_URL: &str = "https://github.com/IdealystIO/idealyst-native";

fn main() {
    // We want a re-build whenever the captured rev would change.
    // `cargo:rerun-if-changed=.git/HEAD` is the conventional way to
    // pick up branch switches; `.git/refs/heads/...` handles commits
    // on the current branch.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    println!("cargo:rerun-if-env-changed=IDEALYST_FRAMEWORK_GIT_URL");
    println!("cargo:rerun-if-env-changed=IDEALYST_FRAMEWORK_GIT_REV");

    let url = std::env::var("IDEALYST_FRAMEWORK_GIT_URL")
        .unwrap_or_else(|_| DEFAULT_URL.to_string());

    // Try the explicit override first; otherwise read git HEAD. If
    // git isn't on PATH or `.git/` isn't present (release tarball
    // build), fall back to the crate version as a tag (`v<version>`).
    let rev = std::env::var("IDEALYST_FRAMEWORK_GIT_REV").ok().unwrap_or_else(|| {
        git_head_sha().unwrap_or_else(|| {
            format!(
                "v{}",
                std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.1".into()),
            )
        })
    });

    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_GIT_URL_DEFAULT={}", url);
    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_GIT_REV_DEFAULT={}", rev);
}

fn git_head_sha() -> Option<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let sha = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if sha.is_empty() { None } else { Some(sha) }
}
