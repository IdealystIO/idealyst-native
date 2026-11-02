//! Every crate whose contributed Kotlin/Java names a class of the framework's
//! own Kotlin runtime (`io.idealyst.runtime.*`) must depend on
//! `backend-android-mobile` for Android.
//!
//! # The bug this prevents
//!
//! The run tool compiles an SDK's `runtime_kotlin` / `runtime_java` files
//! together with the `runtime/kotlin/` tree of whichever
//! `backend-android-mobile` the app's lockfile resolved (see
//! `kotlin_runtime.rs`). Nothing on the Kotlin side stops an SDK from naming a
//! runtime class that only a NEWER backend ships — and if the SDK's
//! `Cargo.toml` doesn't depend on the backend at all, no version requirement
//! stops cargo from keeping the old one. `permissions 3.0.1` called
//! `io.idealyst.runtime.RustPermissionsResult` (added in
//! `backend-android-mobile 4.1.2`) without declaring the dependency, so
//! `cargo update -p permissions` on a lock holding backend 4.1.1 failed in
//! kotlinc with `unresolved reference 'RustPermissionsResult'`.
//!
//! Declaring the dependency through `workspace = true` gives it the
//! sibling's full current version as its floor (the registry tool keeps
//! `[workspace.dependencies]` at full versions), so the published SDK
//! requires a backend at least as new as the one it was built against.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const BACKEND: &str = "backend-android-mobile";
const RUNTIME_PACKAGE_PREFIX: &str = "io.idealyst.runtime.";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..").canonicalize().unwrap()
}

/// `true` when a dependency entry from `cargo metadata` applies to an Android
/// build: unconditional, or behind a target cfg that names android.
fn applies_to_android(dep: &Value) -> bool {
    match dep["target"].as_str() {
        None => true,
        Some(t) => t.contains("android"),
    }
}

fn declared_jvm_files(pkg: &Value) -> Vec<String> {
    let android = &pkg["metadata"]["idealyst"]["android"];
    ["runtime_kotlin", "runtime_java"]
        .iter()
        .flat_map(|key| android[*key].as_array().cloned().unwrap_or_default())
        .filter_map(|v| v.as_str().map(str::to_owned))
        .collect()
}

#[test]
fn regression_sdk_kotlin_naming_runtime_classes_depends_on_backend() {
    let root = workspace_root();
    let out = Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(&root)
        .output()
        .expect("run cargo metadata");
    assert!(out.status.success(), "cargo metadata failed: {}", String::from_utf8_lossy(&out.stderr));
    let meta: Value = serde_json::from_slice(&out.stdout).unwrap();

    let mut checked = 0;
    let mut offenders = Vec::new();
    for pkg in meta["packages"].as_array().unwrap() {
        let name = pkg["name"].as_str().unwrap();
        if name == BACKEND {
            continue;
        }
        let manifest_dir = Path::new(pkg["manifest_path"].as_str().unwrap()).parent().unwrap().to_path_buf();
        let referencing: Vec<String> = declared_jvm_files(pkg)
            .into_iter()
            .filter(|rel| {
                let src = std::fs::read_to_string(manifest_dir.join(rel))
                    .unwrap_or_else(|e| panic!("{name}: declared JVM source `{rel}` unreadable: {e}"));
                src.contains(RUNTIME_PACKAGE_PREFIX)
            })
            .collect();
        if referencing.is_empty() {
            continue;
        }
        checked += 1;
        let depends = pkg["dependencies"]
            .as_array()
            .unwrap()
            .iter()
            .any(|d| d["name"] == BACKEND && d["kind"].is_null() && applies_to_android(d));
        if !depends {
            offenders.push(format!("{name} ({})", referencing.join(", ")));
        }
    }

    assert!(checked > 0, "found no crate whose Kotlin names a runtime class — the scan is broken");
    assert!(
        offenders.is_empty(),
        "these crates' Kotlin/Java names `{RUNTIME_PACKAGE_PREFIX}*` classes but the crate does not \
         depend on `{BACKEND}` for Android, so cargo can resolve a backend that lacks the class:\n  {}\n\
         Add `{BACKEND} = {{ workspace = true }}` under \
         `[target.'cfg(target_os = \"android\")'.dependencies]`.",
        offenders.join("\n  ")
    );
}
