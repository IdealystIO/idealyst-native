//! `idealyst run-linked` — the runner `idealyst dev` hands `cargo run`
//! for a full-stack project's server (and jobs worker), so the process it
//! starts is the binary the LINKER wrote rather than cargo's fresh copy
//! of it.
//!
//! ## Why
//!
//! Cargo "uplifts" a binary from `deps/<crate>-<hash>` to `<profile>/<bin>`
//! on every `cargo build` and `cargo run`, fresh or not, and on macOS it
//! does so by COPYING (a clone, new inode), not hard-linking — hard links
//! to a just-linked binary raced Gatekeeper and got processes killed
//! (rust-lang/cargo#10060). macOS assesses every new executable file the
//! first time it is exec'd, and the assessment reads the whole file:
//! CrewForge's 527 MB debug server sat at `_dyld_start` for 7.5–10 s on
//! every dev session start, server unchanged, while `XprotectService`
//! scanned it. Exec'ing the same file again costs nothing (0.12 s), and
//! the linker's output keeps its inode until the server really relinks.
//! Measured on that binary: `deps/` copy, second exec 0.12 s; the
//! uplifted copy after a no-op `cargo build`, 7.47 s.
//!
//! A runner rather than exec'ing the file from the CLI directly: `cargo
//! run` sets the environment the server is used to (`CARGO_MANIFEST_DIR`,
//! `CARGO_PKG_*`, the dynamic-library search path for native libraries a
//! build script produced, `rustc-env` values), and reproducing that by
//! hand is a second, drifting copy of cargo. The runner changes only
//! which of two identical files is executed. Same precedent as
//! `cargo test`, which runs its binaries from `deps/` too.
//!
//! When the linker's output cannot be identified (no `deps/` sibling with
//! the same length and modification time), the runner executes what cargo
//! passed — the behaviour without it.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// The subcommand name, as cargo will invoke it.
pub const SUBCOMMAND: &str = "run-linked";

#[derive(clap::Args, Debug)]
pub struct Args {
    /// The executable cargo would run, then its arguments.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub rest: Vec<String>,
}

pub fn run(args: Args) -> Result<()> {
    let mut rest = args.rest.into_iter();
    let exe = PathBuf::from(rest.next().context("run-linked: no executable given")?);
    let target = linker_output(&exe).unwrap_or_else(|| exe.clone());
    let mut cmd = Command::new(&target);
    cmd.args(rest);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // argv[0] stays the path cargo named, as under a plain `cargo run`.
        cmd.arg0(&exe);
        let err = cmd.exec();
        Err(err).with_context(|| format!("exec {}", target.display()))
    }
    #[cfg(not(unix))]
    {
        let status = cmd
            .status()
            .with_context(|| format!("run {}", target.display()))?;
        std::process::exit(status.code().unwrap_or(1));
    }
}

/// The file the linker wrote that `exe` — cargo's uplifted copy — was
/// made from: `deps/<crate>-<16 hex><ext>` beside it, with the same length
/// and modification time (a clone, a copy and a hard link all keep both).
/// `None` when there is no such file, or more than one that differs.
pub fn linker_output(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?.join("deps");
    let stem = exe.file_stem()?.to_str()?.replace('-', "_");
    let ext = exe
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"));
    let ext = ext.as_deref().unwrap_or("");
    let want = std::fs::metadata(exe).ok()?;
    let want_mtime = want.modified().ok()?;
    let mut found: Option<PathBuf> = None;
    for entry in std::fs::read_dir(&dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(hash) = name
            .strip_prefix(&stem)
            .and_then(|r| r.strip_prefix('-'))
            .and_then(|r| r.strip_suffix(ext))
        else {
            continue;
        };
        if hash.len() != 16 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let Ok(md) = entry.metadata() else { continue };
        if !md.is_file() || md.len() != want.len() || md.modified().ok() != Some(want_mtime) {
            continue;
        }
        if found.is_some() {
            // Two candidates with identical stamps: not worth guessing.
            return None;
        }
        found = Some(entry.path());
    }
    found
}

/// The `--config` value making `idealyst run-linked` cargo's runner for
/// `host`: `target.<host>.runner = ["<idealyst>", "run-linked"]`.
pub fn runner_config(idealyst: &Path, host: &str) -> String {
    // A TOML array of basic strings; a JSON string literal is one.
    let quote = |s: &str| serde_json::to_string(s).expect("a string serializes");
    format!(
        "target.{host}.runner=[{}, {}]",
        quote(&idealyst.display().to_string()),
        quote(SUBCOMMAND)
    )
}

/// Where a runner for `host` is already configured, if anywhere: the
/// `CARGO_TARGET_<HOST>_RUNNER` variable, or a `runner` under a matching
/// `[target.<host>]` or any `[target.'cfg(…)']` table of a cargo config
/// file cargo would read from `cwd` (each ancestor's `.cargo/config.toml`
/// / `.cargo/config`, then `$CARGO_HOME`'s). The dev loop leaves such a
/// project's `cargo run` alone rather than replace the author's runner.
/// A `cfg(…)` table is counted without evaluating it: overriding a
/// runner the author meant for this host is worse than an exec scan.
pub fn configured_runner(
    cwd: &Path,
    cargo_home: Option<&Path>,
    host: &str,
    env: impl Fn(&str) -> Option<String>,
) -> Option<String> {
    let var = format!(
        "CARGO_TARGET_{}_RUNNER",
        host.to_uppercase().replace(['-', '.'], "_")
    );
    if env(&var).is_some_and(|v| !v.is_empty()) {
        return Some(var);
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for dir in cwd.ancestors() {
        files.push(dir.join(".cargo/config.toml"));
        files.push(dir.join(".cargo/config"));
    }
    if let Some(home) = cargo_home {
        files.push(home.join("config.toml"));
        files.push(home.join("config"));
    }
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(doc) = text.parse::<toml::Table>() else {
            continue;
        };
        let Some(targets) = doc.get("target").and_then(|t| t.as_table()) else {
            continue;
        };
        for (key, table) in targets {
            let matches = key == host || key.starts_with("cfg(");
            if matches && table.get("runner").is_some() {
                return Some(format!("{} [target.{key}]", file.display()));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, bytes: &[u8], mtime: std::time::SystemTime) {
        std::fs::write(path, bytes).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    /// Regression: `cargo run` exec'd cargo's fresh copy of the server,
    /// which macOS scans on first exec — 7.5–10 s for CrewForge's debug
    /// server on every session start. The runner finds the linker's own
    /// file, the one whose identity survives a no-op build.
    #[test]
    fn regression_the_runner_execs_the_linker_output_not_cargos_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let debug = tmp.path().join("debug");
        std::fs::create_dir_all(debug.join("deps")).unwrap();
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000);
        let other = t + std::time::Duration::from_secs(5);
        touch(&debug.join("crewforge-server"), b"binary", t);
        touch(
            &debug.join("deps/crewforge_server-aba181ee2de9277e"),
            b"binary",
            t,
        );
        // An older link of the same bin, and the lib's dep-info.
        touch(
            &debug.join("deps/crewforge_server-2e099f7336f2771f"),
            b"old-binary",
            other,
        );
        touch(
            &debug.join("deps/crewforge_server-2e099f7336f2771f.d"),
            b"binary",
            t,
        );
        assert_eq!(
            linker_output(&debug.join("crewforge-server")),
            Some(debug.join("deps/crewforge_server-aba181ee2de9277e"))
        );
    }

    #[test]
    fn no_matching_linker_output_falls_back_to_cargos_path() {
        let tmp = tempfile::tempdir().unwrap();
        let debug = tmp.path().join("debug");
        std::fs::create_dir_all(debug.join("deps")).unwrap();
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000);
        touch(&debug.join("srv"), b"binary", t);
        touch(
            &debug.join("deps/srv-aba181ee2de9277e"),
            b"binary",
            t + std::time::Duration::from_secs(1),
        );
        assert_eq!(linker_output(&debug.join("srv")), None);
        assert_eq!(linker_output(&tmp.path().join("nowhere/srv")), None);
    }

    #[test]
    fn a_windows_executable_keeps_its_extension() {
        let tmp = tempfile::tempdir().unwrap();
        let debug = tmp.path().join("debug");
        std::fs::create_dir_all(debug.join("deps")).unwrap();
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_790_000_000);
        touch(&debug.join("my-srv.exe"), b"pe", t);
        touch(&debug.join("deps/my_srv-0123456789abcdef.exe"), b"pe", t);
        touch(&debug.join("deps/my_srv-0123456789abcdef.pdb"), b"pe", t);
        assert_eq!(
            linker_output(&debug.join("my-srv.exe")),
            Some(debug.join("deps/my_srv-0123456789abcdef.exe"))
        );
    }

    #[test]
    fn the_runner_config_is_a_toml_array() {
        let arg = runner_config(Path::new("/opt/bin/idealyst"), "aarch64-apple-darwin");
        assert_eq!(
            arg,
            r#"target.aarch64-apple-darwin.runner=["/opt/bin/idealyst", "run-linked"]"#
        );
        let (key, value) = arg.split_once('=').unwrap();
        let doc: toml::Table = format!("v = {value}").parse().unwrap();
        assert_eq!(key, "target.aarch64-apple-darwin.runner");
        assert_eq!(doc["v"].as_array().unwrap().len(), 2);
    }

    /// An author's own runner is never replaced.
    #[test]
    fn an_author_runner_is_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("app/crates/main");
        std::fs::create_dir_all(&project).unwrap();
        let none = |_: &str| None;
        assert_eq!(
            configured_runner(&project, None, "aarch64-apple-darwin", none),
            None
        );

        let host = "aarch64-apple-darwin";
        let env =
            |k: &str| (k == "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUNNER").then(|| "x".to_string());
        assert!(configured_runner(&project, None, host, env).is_some());

        std::fs::create_dir_all(tmp.path().join("app/.cargo")).unwrap();
        std::fs::write(
            tmp.path().join("app/.cargo/config.toml"),
            "[target.aarch64-unknown-linux-gnu]\nrustflags = [\"-Cfoo\"]\n",
        )
        .unwrap();
        assert_eq!(
            configured_runner(&project, None, host, none),
            None,
            "rustflags are not a runner"
        );
        std::fs::write(
            tmp.path().join("app/.cargo/config.toml"),
            "[target.'cfg(target_os = \"macos\")']\nrunner = \"sudo\"\n",
        )
        .unwrap();
        assert!(configured_runner(&project, None, host, none).is_some());

        let home = tmp.path().join("cargo-home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::remove_file(tmp.path().join("app/.cargo/config.toml")).unwrap();
        std::fs::write(
            home.join("config.toml"),
            format!("[target.{host}]\nrunner = \"x\"\n"),
        )
        .unwrap();
        assert!(configured_runner(&project, Some(&home), host, none).is_some());
    }
}
