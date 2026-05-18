//! `idealyst rustc-capture` — internal rustc wrapper used by AAS
//! hot-patch mode to capture the exact rustc command cargo runs for
//! each workspace member. On subsequent file edits the host's
//! hot-patch builder replays those captured invocations with
//! `--emit=obj` (instead of `--emit=link`) to produce `.rcgu.o`
//! files for the user crate's tip without rebuilding the
//! framework crates the bin is statically linked against.
//!
//! ## Invocation shape
//!
//! Cargo invokes the wrapper as:
//!
//! ```text
//! <wrapper> <real-rustc> <rustc-args...>
//! ```
//!
//! when `RUSTC_WORKSPACE_WRAPPER` is set. The CLI binary itself is
//! the wrapper — `main.rs` sniffs `IDEALYST_RUSTC_CAPTURE_DIR` and
//! dispatches here before clap argument parsing kicks in.
//!
//! Captures land at `<dir>/<crate-name>.<crate-type>.json`. Best-
//! effort: any failure here falls through to `exec(rustc)` so the
//! user's build still runs. The hot-patch builder noticing a
//! missing capture file is the place where errors surface, not
//! here.

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
/// Mirror this exact shape in
/// `build-aas/src/hotpatch/replay.rs` — both ends are leaf
/// consumers and a shared crate would just be a two-field struct.
#[derive(Serialize, Deserialize)]
pub struct CapturedInvocation {
    pub rustc: String,
    pub args: Vec<String>,
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

    if let Ok(dir) = std::env::var("IDEALYST_RUSTC_CAPTURE_DIR") {
        // Cargo invokes the wrapper twice per crate sometimes
        // (e.g. the metadata pre-pass for pipelined compilation
        // and the actual link pass). We only want the LINK
        // invocation — emit=metadata only is the pipelined check
        // and would replay to a metadata-only output.
        if let Some(crate_name) = extract_crate_name(&rest) {
            if !is_metadata_only(&rest) && !is_print_only(&rest) {
                let crate_type =
                    extract_crate_type(&rest).unwrap_or_else(|| "lib".to_string());
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
                if let Ok(json) = serde_json::to_string_pretty(&captured) {
                    let _ = std::fs::write(out_dir.join(format!("{key}.json")), json);
                }
            }
        }
    }

    let status = Command::new(&rustc).args(&rest).status()?;
    std::process::exit(status.code().unwrap_or(1));
}

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

fn is_metadata_only(args: &[String]) -> bool {
    for a in args {
        if let Some(emits) = a.strip_prefix("--emit=") {
            return emits == "metadata";
        }
    }
    false
}

fn is_print_only(args: &[String]) -> bool {
    args.iter().any(|a| a.starts_with("--print"))
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
