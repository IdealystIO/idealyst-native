//! Which of the project's local packages a web build compiled, read from
//! the build's own cargo messages.
//!
//! The dev loop needs this set to tell a save that can change the served
//! bundle from one that cannot. `cargo metadata` cannot answer that: it
//! resolves the WHOLE workspace with features unified across every
//! member, so in a workspace whose server binary is a member, an
//! endpoint crate's `server` feature (and the server-only crates it
//! pulls in) shows up in the app's closure even though the wasm build,
//! which selects only the app, never compiles them. It also keeps
//! `[target.'cfg(...)'.dependencies]` for every platform. The
//! `compiler-artifact` messages of the build itself are what cargo
//! actually compiled: fresh units are reported too, so a no-op build
//! still lists everything.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// The rustc target the wasm units are compiled for. Cargo writes a
/// `--target` build's units under `<target-dir>/<triple>/`, and host
/// units (proc-macros, build scripts and their dependencies) under
/// `<target-dir>/<profile>/`, which is how the two are told apart.
const WASM_TRIPLE: &str = "wasm32-unknown-unknown";

/// The local (path-sourced) packages one web build compiled, by manifest
/// directory.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalPackages {
    /// Every local package the build compiled anything of, for the wasm
    /// or for the host. A proc-macro crate or a build-dependency is in
    /// here: its edit changes the bundle by changing what the wasm crates
    /// compile to, so a save in it must still rebuild.
    pub all: BTreeSet<PathBuf>,
    /// The local packages with a unit compiled FOR wasm: the crates whose
    /// code is in the served module, and so the only ones a hot patch can
    /// replace.
    pub wasm: BTreeSet<PathBuf>,
}

impl LocalPackages {
    /// Fold a build's cargo JSON lines (`compiler-artifact` and anything
    /// else; the rest are ignored).
    pub fn from_messages<S: AsRef<str>>(messages: &[S]) -> Self {
        let mut out = Self::default();
        for line in messages {
            let Ok(msg) = serde_json::from_str::<serde_json::Value>(line.as_ref()) else { continue };
            if msg["reason"].as_str() != Some("compiler-artifact") {
                continue;
            }
            // `path+file://…` (cargo ≥ 1.77) or `name ver (path+file://…)`.
            let local = msg["package_id"].as_str().is_some_and(|id| id.starts_with("path+") || id.contains("(path+"));
            if !local {
                continue;
            }
            let Some(dir) = msg["manifest_path"].as_str().and_then(|m| Path::new(m).parent()) else { continue };
            let dir = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
            let for_wasm = msg["filenames"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|f| f.as_str())
                .any(|f| Path::new(f).components().any(|c| c == Component::Normal(WASM_TRIPLE.as_ref())));
            if for_wasm {
                out.wasm.insert(dir.clone());
            }
            out.all.insert(dir);
        }
        out
    }

    /// Whether `dir` (a package's manifest directory) was compiled by the
    /// build at all.
    pub fn reaches(&self, dir: &Path) -> bool {
        self.all.contains(dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(id: &str, manifest: &str, kind: &str, file: &str) -> String {
        serde_json::json!({
            "reason": "compiler-artifact",
            "package_id": id,
            "manifest_path": manifest,
            "target": { "kind": [kind], "name": "x" },
            "filenames": [file],
            "fresh": true,
        })
        .to_string()
    }

    /// The CrewForge shape: the app, a screen, a proc-macro crate and a
    /// registry crate are compiled; the server-only crate never appears,
    /// whatever `cargo metadata` says about it.
    #[test]
    fn regression_server_only_crate_is_not_in_the_web_build() {
        let lines = vec![
            artifact(
                "path+file:///w/app#app@0.1.0",
                "/w/app/Cargo.toml",
                "cdylib",
                "/w/t/wasm32-unknown-unknown/debug/app.wasm",
            ),
            artifact(
                "path+file:///w/screen#screen@0.1.0",
                "/w/screen/Cargo.toml",
                "lib",
                "/w/t/wasm32-unknown-unknown/debug/deps/libscreen-1a2b.rlib",
            ),
            artifact(
                "path+file:///w/macros#macros@0.1.0",
                "/w/macros/Cargo.toml",
                "proc-macro",
                "/w/t/debug/deps/libmacros-3c4d.dylib",
            ),
            artifact(
                "registry+https://github.com/rust-lang/crates.io-index#serde@1.0.0",
                "/home/.cargo/registry/src/serde-1.0.0/Cargo.toml",
                "lib",
                "/w/t/wasm32-unknown-unknown/debug/deps/libserde-5e6f.rlib",
            ),
            r#"{"reason":"build-finished","success":true}"#.to_string(),
            "not json".to_string(),
        ];
        let reach = LocalPackages::from_messages(&lines);
        let set = |xs: &[&str]| xs.iter().map(PathBuf::from).collect::<BTreeSet<_>>();
        assert_eq!(reach.all, set(&["/w/app", "/w/screen", "/w/macros"]));
        assert_eq!(reach.wasm, set(&["/w/app", "/w/screen"]), "a proc-macro is compiled for the host");
        assert!(!reach.reaches(Path::new("/w/api-server")));
    }

    /// The pre-1.77 package id spelling is local too.
    #[test]
    fn the_old_package_id_spelling_is_read() {
        let lines = [artifact(
            "screen 0.1.0 (path+file:///w/screen)",
            "/w/screen/Cargo.toml",
            "lib",
            "/w/t/wasm32-unknown-unknown/debug/deps/libscreen-1a2b.rlib",
        )];
        assert!(LocalPackages::from_messages(&lines).wasm.contains(Path::new("/w/screen")));
    }
}
