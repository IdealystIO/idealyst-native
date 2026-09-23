//! Replay captured rustc invocations with `--emit=obj`.
//!
//! Captures are produced by the `rustc-capture` CLI subcommand
//! during the initial fat build (see
//! [`crate::hotpatch::fat_build_env`]). On each source change the
//! host's rebuild loop hands a `<crate-name>` to
//! [`crate::hotpatch::HotPatchBuilder::build`], which calls into
//! here to:
//!
//!  1. Load the on-disk capture file
//!  2. Rewrite `--emit=link[,…]` to `--emit=obj`
//!  3. Spawn rustc with the captured argv + env + cwd
//!  4. Scrape the artifact json messages for emitted `.o` paths
//!
//! Rustc emits one `.rcgu.o` per codegen unit. For a small bin
//! that's usually 1; for a big crate with default `codegen-units
//! = 16` it can be a dozen. The full list is what feeds the
//! stub generator and the patch link.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Mirror of `cli::cmd::rustc_capture::CapturedInvocation`. Lives
/// here too because both crates need to deserialize without
/// taking a dep on each other.
#[derive(Serialize, Deserialize, Debug)]
pub struct CapturedInvocation {
    pub rustc: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: Vec<(String, String)>,
}

/// Find the capture file for `crate_name`. Cargo invokes rustc
/// once per crate (multiple `--crate-type` flags share an
/// invocation), so a crate with `crate-type = ["cdylib", "rlib"]`
/// has one capture file named for whichever crate-type appears
/// first in the rustc argv. We don't try to be clever — just
/// take the first file matching `<crate_name>.*.json`.
///
/// Cargo normalizes `-` to `_` in `--crate-name`, so a Cargo.toml
/// `name = "hot-reload-test"` ends up as `hot_reload_test` in the
/// rustc argv (and therefore in the capture filename). We try the
/// caller's name first, then the underscore-normalized variant.
pub fn find_capture(
    captures_dir: &Path,
    crate_name: &str,
) -> Result<CapturedInvocation> {
    let primary = format!("{}.", crate_name);
    let normalized = format!("{}.", crate_name.replace('-', "_"));
    let mut candidates: Vec<(usize, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(captures_dir)
        .with_context(|| format!("read captures dir {}", captures_dir.display()))?
    {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.ends_with(".json") {
            continue;
        }
        if name.starts_with(&primary) || name.starts_with(&normalized) {
            candidates.push((crate_type_rank(&name), entry.path()));
        }
    }
    // A project with both a library and a binary of the same name — the
    // standard idealyst shape, where `src/main.rs` is one `entry!` line —
    // writes two captures. Directory order is not defined, so picking
    // "the first" picked either, and replaying the BINARY produced a
    // patch containing an `entry!` expansion and none of the app.
    //
    // Rank, then sort: the library is where the components live.
    candidates.sort();
    let found = candidates.into_iter().next().map(|(_, path)| path);
    let path = found.ok_or_else(|| {
        anyhow::anyhow!(
            "no capture for crate `{}` in {} — was the fat build run with \
             RUSTC_WORKSPACE_WRAPPER set?",
            crate_name,
            captures_dir.display()
        )
    })?;
    let data = std::fs::read(&path)
        .with_context(|| format!("read capture {}", path.display()))?;
    serde_json::from_slice(&data)
        .with_context(|| format!("parse capture {}", path.display()))
}

/// Replay rustc, modifying `--emit=` to emit only `.o` files.
/// Returns the absolute paths of every `.o` rustc produced.
pub fn run_rustc_emit_obj(captured: &CapturedInvocation) -> Result<Vec<PathBuf>> {
    run_rustc_emit_obj_with(captured, &[])
}

/// As [`run_rustc_emit_obj`], with extra rustc flags appended.
///
/// The wasm patch build needs `-Crelocation-model=pic`, because a wasm
/// patch is a PIC side module and its data references have to go through
/// a GOT the loader can point at the base's memory. The flag is appended
/// to the REPLAYED argv rather than set in `RUSTFLAGS` on the base build
/// for one reason that is easy to get wrong: cargo folds `RUSTFLAGS` into
/// the `-Cmetadata` it passes each crate, `-Cmetadata` seeds the symbol
/// mangling hash, and a patch whose symbols hash differently from the
/// base's pairs with nothing. Appending here leaves the captured
/// `-Cmetadata` exactly as it was.
pub fn run_rustc_emit_obj_with(
    captured: &CapturedInvocation,
    extra: &[String],
) -> Result<Vec<PathBuf>> {
    // Rewrite emit args in place. The captured args include
    // `--emit=dep-info,link` (or similar) — we replace with
    // `--emit=obj` so rustc skips linking and writes one .rcgu.o
    // per codegen unit.
    //
    // We deliberately keep the rest of the captured argv identical
    // to cargo's: changing flags (e.g. `-Cdebuginfo=0`) keys to a
    // different incremental-cache slot than cargo's, so subsequent
    // replays cold-pay 20-30ms recompile every time. For tiny tip
    // crates this regression outweighs the small per-pass savings
    // from less codegen work.
    let mut args: Vec<String> = Vec::with_capacity(captured.args.len() + 1);
    let mut emit_set = false;
    let mut iter = captured.args.iter();
    while let Some(a) = iter.next() {
        if a == "--emit" {
            let _ = iter.next();
            args.push("--emit=obj".to_string());
            emit_set = true;
            continue;
        }
        if a.starts_with("--emit=") {
            args.push("--emit=obj".to_string());
            emit_set = true;
            continue;
        }
        args.push(a.clone());
    }
    if !emit_set {
        args.push("--emit=obj".to_string());
    }
    args.extend(extra.iter().cloned());

    let mut cmd = Command::new(&captured.rustc);
    cmd.args(&args).current_dir(&captured.cwd);
    cmd.env_clear();
    for (k, v) in &captured.env {
        cmd.env(k, v);
    }

    let output = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .context("spawn rustc")?;
    if !output.status.success() {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &output.stderr);
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &output.stdout);
        anyhow::bail!("rustc --emit=obj exited with {}", output.status);
    }

    // Cargo passes `--json=...artifacts...` so rustc emits
    // artifact notifications. They land on stdout (or stderr,
    // depending on rustc version). Scan both.
    let mut out: Vec<PathBuf> = Vec::new();
    let scan = |bytes: &[u8], out: &mut Vec<PathBuf>| {
        for line in bytes.split(|b| *b == b'\n') {
            if line.is_empty() {
                continue;
            }
            let Ok(val) = serde_json::from_slice::<serde_json::Value>(line) else {
                continue;
            };
            if val.get("$message_type").and_then(|v| v.as_str()) == Some("artifact") {
                if let Some(p) = val.get("artifact").and_then(|v| v.as_str()) {
                    if p.ends_with(".o") {
                        out.push(PathBuf::from(p));
                    }
                }
            }
        }
    };
    scan(&output.stdout, &mut out);
    scan(&output.stderr, &mut out);

    if out.is_empty() {
        // Defensive: rustc may emit objects without artifact
        // messages on some flag combos. Re-scan for sibling .o
        // files in the captured `--out-dir`. The captured args
        // include `--out-dir <DIR>`; pull it out.
        if let Some(dir) = arg_value(&captured.args, "--out-dir") {
            if let Ok(read) = std::fs::read_dir(&dir) {
                for entry in read.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|s| s.to_str()) == Some("o") {
                        out.push(p);
                    }
                }
            }
        }
    }

    Ok(out)
}

/// Preference order among several captures for one crate name: lower is
/// better. The library targets come first because that is where an app's
/// components are defined; a `bin` of the same name is the entry point
/// and holds nothing worth patching.
fn crate_type_rank(file_name: &str) -> usize {
    let stem = file_name.trim_end_matches(".json");
    match stem.rsplit('.').next().unwrap_or("") {
        "rlib" => 0,
        "lib" => 1,
        "cdylib" => 2,
        "staticlib" => 3,
        "bin" => 4,
        _ => 5,
    }
}

fn arg_value(args: &[String], key: &str) -> Option<String> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == key {
            return iter.next().cloned();
        }
        if let Some(rest) = a.strip_prefix(&format!("{key}=")) {
            return Some(rest.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str) {
        let capture = CapturedInvocation {
            rustc: format!("/usr/bin/rustc-{name}"),
            args: vec!["--crate-name".into(), "app".into()],
            cwd: "/tmp".into(),
            env: Vec::new(),
        };
        std::fs::write(
            dir.join(name),
            serde_json::to_string(&capture).unwrap(),
        )
        .unwrap();
    }

    /// Regression: the standard idealyst shape is a library plus a
    /// binary of the SAME name, whose `src/main.rs` is one `entry!`
    /// line. Both get a capture, directory order is undefined, and
    /// taking "the first" replayed the binary about half the time —
    /// producing a patch that contained an `entry!` expansion and none
    /// of the app.
    #[test]
    fn regression_a_lib_and_a_bin_of_one_name_resolve_to_the_lib() {
        let dir = std::env::temp_dir().join(format!(
            "idealyst-capture-rank-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir, "my_app.bin.json");
        write(&dir, "my_app.rlib.json");

        let found = find_capture(&dir, "my-app").unwrap();
        assert_eq!(
            found.rustc, "/usr/bin/rustc-my_app.rlib.json",
            "the library capture is the one with the components in it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A crate with only a binary still resolves — an app whose whole
    /// source is in `main.rs` is unusual but legal.
    #[test]
    fn a_bin_only_crate_still_resolves() {
        let dir = std::env::temp_dir().join(format!(
            "idealyst-capture-binonly-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        write(&dir, "solo.bin.json");
        assert!(find_capture(&dir, "solo").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
