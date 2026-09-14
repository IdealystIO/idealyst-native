//! A backend that ships a two-axis grid engine must actually hand it to
//! `caps::GridOps`.
//!
//! Every `caps::*Ops` method has a default, which is what lets a new
//! capability land without breaking eleven backends at once. The cost of
//! that design is this failure mode: an `impl caps::GridOps for XBackend
//! {}` is valid Rust, compiles clean, raises no warning — and turns every
//! `virtual_grid` in every app on that platform into
//! `missing_primitive_placeholder`, a grey box reading "not supported".
//! The engine behind it can be finished, tested and documented and
//! nobody finds out.
//!
//! That is not hypothetical. Both mobile backends carried a complete
//! `virtual_grid` implementation — the iOS `UIScrollView` engine, the
//! Android `RustVirtualGrid` with its JNI exports and Kotlin view, each
//! with its `create_virtual_grid_impl` / `data_changed` / `release` /
//! `make_handle` quartet on the backend struct — behind an EMPTY
//! `GridOps` impl carrying the comment "no two-axis grid engine on this
//! backend yet". A CrewForge schedule grid on an iPhone rendered the
//! placeholder and nothing else.
//!
//! Whether a trait method is defaulted or overridden is not observable
//! at runtime — a defaulted method is simply the one that runs — so this
//! guards the source, in the same spirit and with the same idiom as
//! `boot_seam_surface.rs` and `premint_only_surface.rs`.
//!
//! The rule is derived, not listed: a crate that names
//! `create_virtual_grid_impl` has an engine, and a crate with an engine
//! must override `create_virtual_grid` in its `GridOps` impl. Backends
//! with no engine (terminal, email, SSR, …) default the whole trait and
//! are right to. Nothing has to be added here when a backend grows one.

use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("repo root")
        .to_path_buf()
}

/// `crates/backend/<name>/src/newcore.rs` and its nested-crate form,
/// paired with the crate root the engine would live under.
fn backend_crates() -> Vec<(String, PathBuf)> {
    let backends = repo_root().join("crates/backend");
    let mut out = Vec::new();
    let mut push = |dir: PathBuf| {
        if dir.join("src/newcore.rs").is_file() {
            let name = dir
                .strip_prefix(repo_root().join("crates/backend"))
                .expect("under crates/backend")
                .to_string_lossy()
                .into_owned();
            out.push((name, dir));
        }
    };
    for entry in std::fs::read_dir(&backends).expect("crates/backend") {
        let dir = entry.expect("dir entry").path();
        if !dir.is_dir() {
            continue;
        }
        push(dir.clone());
        // Mobile backends nest one deeper (`ios/mobile`, `android/mobile`).
        for nested in std::fs::read_dir(&dir).expect("backend dir").flatten() {
            let nested = nested.path();
            if nested.is_dir() {
                push(nested);
            }
        }
    }
    out
}

/// Every `.rs` under the crate, concatenated. Crude and fast enough:
/// the question is only whether a name appears anywhere in it.
fn crate_sources(root: &Path) -> String {
    fn walk(dir: &Path, out: &mut String) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push_str(&std::fs::read_to_string(&path).unwrap_or_default());
            }
        }
    }
    let mut out = String::new();
    walk(&root.join("src"), &mut out);
    out
}

#[test]
fn a_backend_with_a_grid_engine_wires_it_into_grid_ops() {
    let mut engines = Vec::new();
    for (name, root) in backend_crates() {
        let sources = crate_sources(&root);
        if !sources.contains("create_virtual_grid_impl") {
            continue;
        }
        engines.push(name.clone());
        let newcore = std::fs::read_to_string(root.join("src/newcore.rs")).expect("newcore.rs");
        // The empty impl is the defect, in either spelling the
        // formatter produces.
        let empty = newcore.contains("impl caps::GridOps for")
            && !newcore.contains("fn create_virtual_grid(");
        assert!(
            !empty,
            "backend `{name}` ships a virtual_grid engine \
             (`create_virtual_grid_impl`) but its `caps::GridOps` impl is \
             empty, so every method defaults and `virtual_grid` reports \
             itself unsupported on that platform. The engine is dead code \
             reachable by nothing; wire the four methods through to it the \
             way macOS does.",
        );
    }
    // A rename of `create_virtual_grid_impl` would make the loop above
    // match nothing and pass vacuously — which is precisely the silence
    // this file exists to break.
    assert!(
        engines.len() >= 3,
        "expected the iOS, Android and macOS grid engines to be found by \
         their `create_virtual_grid_impl`; found {engines:?}. If the \
         naming convention changed, change the probe with it rather than \
         letting this test pass on an empty set",
    );
}
