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
    let mut found: Option<PathBuf> = None;
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
            found = Some(entry.path());
            break;
        }
    }
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
