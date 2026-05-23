//! Bake compile-time git defaults into the `idealyst` binary.
//!
//! The CLI scaffolds wrapper Cargo.tomls that depend on the framework
//! crates. When the project lives outside the framework workspace
//! (the `cargo install idealyst-cli` case), those deps need a git
//! URL + refspec. We capture both at *CLI* compile time so a given
//! installed binary always scaffolds projects pinned to the framework
//! commit it was built against — predictable, reproducible, no
//! "default branch moved underneath me" surprises.
//!
//! Refspec preference, in order:
//! 1. `IDEALYST_FRAMEWORK_GIT_TAG` env var → `tag = "<value>"`
//! 2. `IDEALYST_FRAMEWORK_GIT_REV` env var → `rev = "<value>"`
//! 3. Git tag at HEAD (from `git describe --tags --exact-match HEAD`)
//!    → `tag = "<value>"`. Preferred over rev because tags are
//!    human-readable and stable.
//! 4. Git commit SHA → `rev = "<value>"`.
//! 5. `v<CARGO_PKG_VERSION>` as a final fallback (used in source
//!    tarballs where `.git/` isn't present).
//!
//! URL is overridable via `IDEALYST_FRAMEWORK_GIT_URL` at both build
//! and runtime; defaults to the public idealyst-native repo.

use std::process::Command;

const DEFAULT_URL: &str = "https://github.com/IdealystIO/idealyst-native";

fn main() {
    // Watch the reflog — it appends on every HEAD movement (commit,
    // amend, reset, checkout, merge, rebase, …). The previous list
    // — `HEAD`, `index`, `refs/tags` — never updated on a regular
    // commit (commits move `refs/heads/<branch>`, not `HEAD`
    // itself), so build.rs's baked SHA went stale until a
    // `cargo install --force` blew the cache. Watching the reflog
    // fixes that: every `git commit` mutates `logs/HEAD`, cargo
    // re-runs build.rs, the new SHA gets baked.
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/logs/HEAD");
    println!("cargo:rerun-if-changed=../../.git/refs/tags");
    println!("cargo:rerun-if-env-changed=IDEALYST_FRAMEWORK_GIT_URL");
    println!("cargo:rerun-if-env-changed=IDEALYST_FRAMEWORK_GIT_REV");
    println!("cargo:rerun-if-env-changed=IDEALYST_FRAMEWORK_GIT_TAG");

    let url = std::env::var("IDEALYST_FRAMEWORK_GIT_URL")
        .unwrap_or_else(|_| DEFAULT_URL.to_string());

    let (kind, value) = resolve_refspec();

    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_GIT_URL_DEFAULT={}", url);
    // Two env consts so the runtime can pick the right TOML key.
    // `KIND` is one of `rev`, `tag`, `branch` (the cargo dep-table
    // key). `VALUE` is the corresponding string.
    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_GIT_REF_KIND_DEFAULT={}", kind);
    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_GIT_REF_VALUE_DEFAULT={}", value);
    // Legacy compat: keep the old `_REV_` constant pointing at
    // whatever value we picked, so older builds that imported it
    // don't break mid-upgrade. New code reads the KIND + VALUE pair.
    println!("cargo:rustc-env=IDEALYST_FRAMEWORK_GIT_REV_DEFAULT={}", value);
}

/// Returns `(refspec_kind, refspec_value)` where `refspec_kind` is
/// `"rev"`, `"tag"`, or `"branch"`.
fn resolve_refspec() -> (&'static str, String) {
    if let Ok(tag) = std::env::var("IDEALYST_FRAMEWORK_GIT_TAG") {
        if !tag.is_empty() {
            return ("tag", tag);
        }
    }
    if let Ok(rev) = std::env::var("IDEALYST_FRAMEWORK_GIT_REV") {
        if !rev.is_empty() {
            return ("rev", rev);
        }
    }
    if let Some(tag) = git_head_tag() {
        return ("tag", tag);
    }
    if let Some(sha) = git_head_sha() {
        return ("rev", sha);
    }
    (
        "tag",
        format!(
            "v{}",
            std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.1".into()),
        ),
    )
}

/// Tag at exact HEAD, if any. `git describe --tags --exact-match HEAD`
/// errors when HEAD isn't tagged; we treat that as "no tag" and
/// return None.
fn git_head_tag() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--tags", "--exact-match", "HEAD"])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let tag = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if tag.is_empty() { None } else { Some(tag) }
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
