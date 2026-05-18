//! `idealyst rebuild-patch <mode-dir>` — fast rebuild for AAS dylib
//! hot-reload.
//!
//! Replays the captured rustc invocations for the user crate
//! (`docs`) and the patch crate (`patch`) directly, bypassing cargo.
//! This skips cargo's overhead (fingerprint check + dep graph walk,
//! ~150 ms) AND skips relinking the host bin entirely (~200 ms) —
//! the host bin's source doesn't change during a session, so any
//! reason cargo finds to relink it is wasted.
//!
//! Steady-state hot-reload cycle target with this in place is
//! ~300–400 ms per edit, vs. the ~750 ms we get with `cargo build
//! -p host -p patch`.
//!
//! The capture step that populates `<mode-dir>/.rustc-args/` runs
//! once during the initial `idealyst build --aas`; see
//! [`crate::cmd::rustc_capture`].

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::cmd::rustc_capture::CapturedInvocation;

#[derive(clap::Args, Debug)]
pub struct Args {
    /// AAS dylib sub-workspace dir, e.g.
    /// `target/idealyst/<project>/aas/dylib-mode`. The `.rustc-args/`
    /// subdir under here holds the captured rustc invocations.
    pub mode_dir: PathBuf,
}

pub fn run(args: Args) -> Result<()> {
    let capture_dir = args.mode_dir.join(".rustc-args");
    if !capture_dir.is_dir() {
        anyhow::bail!(
            "no captured rustc args at {} — was the initial AAS build run with the rustc-capture wrapper?",
            capture_dir.display()
        );
    }

    // Run the user crate's rustc first (rebuilds docs's rlib +
    // cdylib), then the patch crate's rustc (relinks
    // libpatch.dylib against the new docs.rlib). Order matters: the
    // patch link picks up `libdocs.rlib` by canonical (no-hash)
    // name, which cargo updates as a hardlink to the latest
    // compilation — so rerunning docs's rustc first is enough.
    //
    // Anything else in the capture dir (framework-core,
    // framework-hot, idea-ui, host bin) is intentionally NOT
    // replayed here. Those crates are stable across edits within a
    // session, and re-running rustc on them would just produce
    // bit-identical output at extra cost.
    let docs_capture = find_capture(&capture_dir, "docs")
        .with_context(|| "locating docs's captured rustc invocation")?;
    replay(&docs_capture).with_context(|| "replay docs rustc")?;

    let patch_capture = find_capture(&capture_dir, "patch")
        .with_context(|| "locating patch's captured rustc invocation")?;
    replay(&patch_capture).with_context(|| "replay patch rustc")?;

    Ok(())
}

/// Locate the JSON capture for a given crate. Captures are keyed as
/// `<crate-name>.<crate-type>.json`; we accept the first match by
/// prefix because a crate can have multiple crate-types (e.g. docs
/// is `cdylib` + `rlib`, but one rustc call emits both so only one
/// json is written).
fn find_capture(capture_dir: &Path, crate_name: &str) -> Result<PathBuf> {
    let prefix = format!("{crate_name}.");
    for entry in std::fs::read_dir(capture_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(&prefix) && name.ends_with(".json") {
            return Ok(entry.path());
        }
    }
    anyhow::bail!(
        "no capture for crate `{crate_name}` in {}",
        capture_dir.display()
    )
}

fn replay(capture_path: &Path) -> Result<()> {
    let data = std::fs::read(capture_path)
        .with_context(|| format!("read {}", capture_path.display()))?;
    let captured: CapturedInvocation = serde_json::from_slice(&data)
        .with_context(|| format!("parse {}", capture_path.display()))?;

    let mut cmd = Command::new(&captured.rustc);
    cmd.args(&captured.args).current_dir(&captured.cwd);
    // Clear env first — captured invocations carry the precise env
    // cargo built up (CARGO_PKG_NAME, OUT_DIR, RUSTFLAGS, etc.).
    // Forwarding the current shell's env on top would let user
    // overrides bleed in.
    cmd.env_clear();
    for (k, v) in &captured.env {
        cmd.env(k, v);
    }

    let status = cmd
        .status()
        .with_context(|| format!("spawn rustc for {}", capture_path.display()))?;
    if !status.success() {
        anyhow::bail!("rustc exited with {status} for {}", capture_path.display());
    }
    Ok(())
}
