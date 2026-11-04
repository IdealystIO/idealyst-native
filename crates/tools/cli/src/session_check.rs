//! The cargo arguments of a dev session's test-harness check
//! (`idealyst check --session --tests -p <crate>`).
//!
//! The harness has to reuse the units the session's server build left in its
//! target dir, and a unit's hash carries its resolved features. Cargo resolves
//! features over the workspace members SELECTED on its command line, so `-p <crate>`
//! alone can turn a dependency's features off and recompile it and everything
//! above it (CrewForge measured 344, 36 and 2 crates compiled by naming
//! different packages). So the server's package stays selected, and the
//! target filters below keep it from adding work: `--lib`, `--bin` and
//! `--test` apply across the selection, and the server's own binary matches
//! none of them.
//!
//! `cargo test --no-run`, not `cargo build --tests`: the latter compiles a
//! library in build mode, without its `#[cfg(test)]` modules.
//!
//! What can still move: `cargo test` turns on dev-dependencies, and one that
//! enables a feature of a shared dependency changes that dependency's hash.
//! The report's `compiled` then lists more than the crate, and
//! `idealyst check` says why. A server package that has a library of its own
//! gets that library's harness compiled too: `--lib` cannot be aimed at one
//! package.

use std::path::Path;

/// The arguments after `cargo test --no-run`, without the target dir, for the
/// test harness of `krate` beside the server `base_args` selects.
///
/// `base_args` is `server_build_args`'s: `--manifest-path <m>` or `-p <pkg>
/// --features server`, then `--bin <b>`. `meta` is `cargo metadata` of
/// `server_manifest`, which names the server's package and `krate`'s targets.
pub(crate) fn test_build_args(
    base_args: &[String],
    meta: &serde_json::Value,
    server_manifest: &Path,
    krate: &str,
) -> Result<Vec<String>, String> {
    let packages = meta["packages"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let local = packages.iter().find(|p| p["name"] == krate && p["source"].is_null()).ok_or_else(|| {
        format!(
            "`{krate}` is not a local package of the server's build, so the session's server target dir \
             holds nothing for it. Test modules compile beside the server; a crate only the web bundle uses \
             has no host build to reuse"
        )
    })?;
    let manifest = std::fs::canonicalize(server_manifest).unwrap_or_else(|_| server_manifest.to_path_buf());
    let server = packages
        .iter()
        .find(|p| p["manifest_path"].as_str().is_some_and(|m| same_path(Path::new(m), &manifest)))
        .and_then(|p| p["name"].as_str())
        .ok_or_else(|| format!("cargo metadata names no package at {}", manifest.display()))?;

    let mut args = Vec::new();
    let mut base = base_args.iter();
    let mut selects_features = false;
    while let Some(arg) = base.next() {
        match arg.as_str() {
            "--manifest-path" => {
                args.push(arg.clone());
                args.extend(base.next().cloned());
            }
            // Spelled through the package below: with two packages selected,
            // a bare `server` would also turn on a same-named feature of
            // `krate`, which the session's build did not.
            "--features" => {
                base.next();
                selects_features = true;
            }
            _ => {}
        }
    }
    args.extend(["-p".to_string(), krate.to_string()]);
    if krate != server {
        args.extend(["-p".to_string(), server.to_string()]);
    }
    if selects_features {
        args.extend(["--features".to_string(), format!("{server}/server")]);
    }

    let mut filters = Vec::new();
    for t in local["targets"].as_array().into_iter().flatten() {
        if t["test"] == false {
            continue;
        }
        let name = t["name"].as_str().unwrap_or_default().to_string();
        let kinds: Vec<&str> = t["kind"].as_array().into_iter().flatten().filter_map(|k| k.as_str()).collect();
        if kinds.iter().any(|k| matches!(*k, "lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro")) {
            filters.push("--lib".to_string());
        } else if kinds.contains(&"bin") {
            filters.extend(["--bin".to_string(), name]);
        } else if kinds.contains(&"test") {
            filters.extend(["--test".to_string(), name]);
        }
    }
    if filters.is_empty() {
        return Err(format!("`{krate}` has no test targets"));
    }
    args.extend(filters);
    Ok(args)
}

fn same_path(a: &Path, b: &Path) -> bool {
    std::fs::canonicalize(a).map_or_else(|_| a == b, |a| a == b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn target(kind: &str, name: &str) -> serde_json::Value {
        json!({ "kind": [kind], "name": name, "test": true })
    }

    fn meta() -> serde_json::Value {
        json!({ "packages": [
            { "name": "server-bin", "source": null, "manifest_path": "/w/server-bin/Cargo.toml",
              "targets": [target("bin", "crewforge-server")] },
            { "name": "core", "source": null, "manifest_path": "/w/core/Cargo.toml",
              "targets": [target("lib", "core"), target("test", "flows"), target("example", "demo"),
                          json!({ "kind": ["bin"], "name": "seed", "test": false })] },
            { "name": "serde", "source": "registry+https://x", "manifest_path": "/r/serde/Cargo.toml",
              "targets": [target("lib", "serde")] },
            { "name": "tools", "source": null, "manifest_path": "/w/tools/Cargo.toml",
              "targets": [json!({ "kind": ["example"], "name": "x", "test": false })] },
        ]})
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| s.to_string()).collect()
    }

    /// Feature unification: the server stays selected, and its binary is not
    /// built. Examples and a `test = false` target are not test modules.
    #[test]
    fn a_test_build_keeps_the_server_selected_and_builds_only_the_crates_tests() {
        let base = strings(&["--manifest-path", "/w/server-bin/Cargo.toml", "--bin", "crewforge-server"]);
        let args = test_build_args(&base, &meta(), Path::new("/w/server-bin/Cargo.toml"), "core").unwrap();
        assert_eq!(
            args,
            strings(&[
                "--manifest-path",
                "/w/server-bin/Cargo.toml",
                "-p",
                "core",
                "-p",
                "server-bin",
                "--lib",
                "--test",
                "flows",
            ])
        );
    }

    /// The in-crate shape selects by package and feature. The feature is
    /// spelled through the server's package, so it cannot reach `krate`.
    #[test]
    fn the_in_crate_servers_feature_stays_on_its_own_package() {
        let base = strings(&["-p", "server-bin", "--features", "server", "--bin", "crewforge-server"]);
        let args = test_build_args(&base, &meta(), Path::new("/w/server-bin/Cargo.toml"), "core").unwrap();
        assert_eq!(args[..6], strings(&["-p", "core", "-p", "server-bin", "--features", "server-bin/server"]));
    }

    /// The server's own tests: one package, and its binary's harness only.
    #[test]
    fn the_server_package_itself_is_selected_once() {
        let base = strings(&["--manifest-path", "/w/server-bin/Cargo.toml", "--bin", "crewforge-server"]);
        let args = test_build_args(&base, &meta(), Path::new("/w/server-bin/Cargo.toml"), "server-bin").unwrap();
        assert_eq!(
            args,
            strings(&["--manifest-path", "/w/server-bin/Cargo.toml", "-p", "server-bin", "--bin", "crewforge-server"])
        );
    }

    #[test]
    fn what_has_no_test_build_here_is_refused_in_words() {
        let base = strings(&["--manifest-path", "/w/server-bin/Cargo.toml"]);
        let m = Path::new("/w/server-bin/Cargo.toml");
        for krate in ["serde", "web-only"] {
            let e = test_build_args(&base, &meta(), m, krate).unwrap_err();
            assert!(e.contains("not a local package"), "{e}");
        }
        assert!(test_build_args(&base, &meta(), m, "tools").unwrap_err().contains("no test targets"));
    }
}
