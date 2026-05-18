//! `idealyst rustc-capture` — internal rustc wrapper used by AAS
//! dylib mode to capture the exact rustc command cargo runs for the
//! patch crate (and the user crate). On subsequent edits, the host's
//! watcher loop replays the captured commands directly with bare
//! rustc, bypassing cargo's fingerprint / dep-graph / host-relink
//! overhead. That's the difference between a ~750ms cargo rebuild
//! and a ~250ms direct rustc invocation.
//!
//! ## Invocation shape
//!
//! Cargo invokes the wrapper as:
//!
//! ```text
//! <wrapper> <real-rustc> <rustc-args...>
//! ```
//!
//! when `RUSTC_WRAPPER` or `RUSTC_WORKSPACE_WRAPPER` is set to this
//! subcommand. We honor `IDEALYST_RUSTC_CAPTURE_DIR` (set by the
//! initial AAS build) — if present, we serialize the rustc argv
//! (plus a small set of relevant env vars + cwd) to
//! `<capture-dir>/<crate-name>.json` and then exec the real rustc.
//! If unset, we just exec rustc unchanged.

use std::path::PathBuf;
use std::process::Command;

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(clap::Args, Debug)]
pub struct Args {
    /// Path to the real rustc + every arg cargo passed. The first
    /// element is rustc itself; the rest are its args.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    pub rest: Vec<String>,
}

/// On-disk capture record. JSON for easy editing during debugging.
#[derive(Serialize, Deserialize)]
pub struct CapturedInvocation {
    /// Absolute path to the real rustc binary.
    pub rustc: String,
    /// rustc's argv (without rustc itself).
    pub args: Vec<String>,
    /// Working directory cargo invoked rustc from. Some `--out-dir`
    /// values are relative.
    pub cwd: String,
    /// Subset of env vars rustc cares about. We capture everything
    /// starting with `CARGO`, `RUST`, `IDEALYST`, plus `OUT_DIR`,
    /// `PATH`, and a few well-known build-script outputs.
    pub env: Vec<(String, String)>,
}

pub fn run(args: Args) -> Result<()> {
    let mut iter = args.rest.into_iter();
    let rustc = iter
        .next()
        .ok_or_else(|| anyhow::anyhow!("rustc-capture: no rustc path supplied"))?;
    let rest: Vec<String> = iter.collect();

    // If the capture dir isn't set, the wrapper is a pass-through —
    // ordinary `idealyst build` / `cargo check` invocations don't
    // need capture overhead.
    if let Ok(dir) = std::env::var("IDEALYST_RUSTC_CAPTURE_DIR") {
        if let Some(crate_name) = extract_crate_name(&rest) {
            let target = std::env::var("CARGO_PRIMARY_PACKAGE").ok();
            // Cargo invokes rustc once per (crate, target) pair, so
            // include `rlib` / `dylib` / `bin` in the key.
            let crate_type = extract_crate_type(&rest).unwrap_or_else(|| "lib".to_string());
            let key = format!("{}.{}", crate_name, crate_type);
            let out_dir = PathBuf::from(&dir);
            let _ = std::fs::create_dir_all(&out_dir);

            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            let env: Vec<(String, String)> = std::env::vars()
                .filter(|(k, _)| relevant_env(k))
                .collect();

            let captured = CapturedInvocation {
                rustc: rustc.clone(),
                args: rest.clone(),
                cwd,
                env,
            };
            let json = serde_json::to_string_pretty(&captured)?;
            let file = out_dir.join(format!("{key}.json"));
            // Best-effort write — if anything fails we still want to
            // pass through to rustc so the user's build doesn't die.
            let _ = std::fs::write(&file, json);
            let _ = target; // future: dedup by primary-package status
        }
    }

    // Pass through to the real rustc. We use `status()` rather than
    // `exec()` so any post-write bookkeeping above completes first,
    // and so any non-Unix platform gets the same code path.
    let status = Command::new(&rustc).args(&rest).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

/// Pull `--crate-name <NAME>` out of rustc's argv.
fn extract_crate_name(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == "--crate-name" {
            return iter.next().cloned();
        }
        if let Some(rest) = a.strip_prefix("--crate-name=") {
            return Some(rest.to_string());
        }
    }
    None
}

/// Pull `--crate-type <T>` out. Cargo can pass it multiple times
/// (rlib + dylib for hybrid crates); the FIRST is good enough as a
/// disambiguator.
fn extract_crate_type(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(a) = iter.next() {
        if a == "--crate-type" {
            return iter.next().cloned();
        }
        if let Some(rest) = a.strip_prefix("--crate-type=") {
            return Some(rest.to_string());
        }
    }
    None
}

fn relevant_env(key: &str) -> bool {
    key.starts_with("CARGO")
        || key.starts_with("RUST")
        || key.starts_with("IDEALYST")
        || matches!(
            key,
            "OUT_DIR" | "PATH" | "DYLD_LIBRARY_PATH" | "LD_LIBRARY_PATH" | "HOME"
        )
}
