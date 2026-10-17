//! Every thread-local in a crate an Android app links is declared with
//! `runtime_tls::thread_local!`, not std's.
//!
//! On Android a std `thread_local!` static takes its own pthread key (the
//! target has no native thread-locals) and bionic aborts past 128. With
//! std's, the robot-e2e demo aborted on its first real touch — in
//! `user_activity::LISTENERS`, then in the async executor's task table —
//! whichever static happened to reach for the key past the limit. The
//! framework declares a few hundred; this keeps a new one from going back
//! to std's.

use std::path::{Path, PathBuf};

/// The crates an Android app links that declare thread-locals: the
/// runtime, the Android backend, the UI kits, the facade and every client
/// SDK. Web-only and host-only crates (web-glue, the proc macros, tools)
/// are not linked into an Android app.
fn android_linked_crates(root: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["runtime/world", "runtime/shared", "runtime/scene", "runtime/layout", "runtime/vocabulary",
        "runtime/template", "runtime/core", "idealyst", "backend/android/mobile", "backend/android/core",
        "backend/android/tv", "ui/idea-theme", "ui/idea-ui", "ui/idea-ui-nav"]
        .iter()
        .map(|d| root.join("crates").join(d))
        .collect();
    // Client SDKs, including the ones grouped one or two directories down
    // (`navigators/stack`, `navigators/helpers/android`): a group directory
    // has no Cargo.toml of its own, so only collecting direct children
    // skipped every crate inside it.
    fn crates_under(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if !path.is_dir() {
                continue;
            }
            if path.join("Cargo.toml").exists() {
                out.push(path);
            } else if depth > 0 {
                crates_under(&path, depth - 1, out);
            }
        }
    }
    crates_under(&root.join("crates/sdk/client"), 2, &mut dirs);
    dirs.retain(|d| d.join("Cargo.toml").exists());
    dirs
}

#[test]
fn the_scan_reaches_crates_inside_sdk_groups() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let dirs = android_linked_crates(&root);
    for nested in ["navigators/stack", "navigators/helpers/android"] {
        let want = root.join("crates/sdk/client").join(nested);
        assert!(dirs.contains(&want), "{nested} is scanned");
    }
}

fn scan(dir: &Path, hits: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            scan(&path, hits);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let src = std::fs::read_to_string(&path).unwrap();
            for (n, line) in src.lines().enumerate() {
                let code = line.trim_start();
                if code.starts_with("//") {
                    continue;
                }
                // An invocation: `thread_local!`, `std::thread_local!` or
                // `::std::thread_local!` not qualified by `runtime_tls::`.
                if let Some(at) = code.find("thread_local!") {
                    let before = &code[..at];
                    if !before.ends_with("runtime_tls::") && (before.is_empty() || before.ends_with("std::")) {
                        hits.push(format!("{}:{}: {}", path.display(), n + 1, code));
                    }
                }
            }
        }
    }
}

#[test]
fn android_linked_crates_declare_thread_locals_with_runtime_tls() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut hits = Vec::new();
    for dir in android_linked_crates(&root) {
        scan(&dir.join("src"), &mut hits);
    }
    assert!(
        hits.is_empty(),
        "std thread_local! in an Android-linked crate — each costs one of bionic's 128 pthread keys; \
         write `runtime_tls::thread_local!` (and depend on runtime-tls):\n{}",
        hits.join("\n")
    );
}
