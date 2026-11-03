//! Invoke the system linker directly (no `cc` driver) to produce
//! the patch dylib.
//!
//! Output is a single `.dylib` whose external references are
//! either (a) resolved by the stub trampolines to absolute host
//! addresses, or (b) lazy-bound at dlopen against libSystem via
//! `-undefined dynamic_lookup`.
//!
//! We invoke `ld` directly rather than `cc -dynamiclib` to skip
//! the cc/clang driver's argv shuffle and the second subprocess
//! spawn. On macOS each `Command::new` is ~25 ms; chaining cc
//! into ld doubles that. Going straight to ld trims one spawn
//! out of the per-patch hot path.
//!
//! v1 is macOS aarch64. Linux + Windows + wasm follow the same
//! shape but differ in linker flags + invocation. The `ld -v`
//! output here is Apple's classic ld64 (project ld-1267+); the
//! arg set tracks what `cc -dynamiclib` would have passed to it
//! after its own flag translation.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

/// SDK path cache. `xcrun --show-sdk-path` shells out and takes
/// ~50 ms — too expensive to run per patch. The path is constant
/// for the dev session (it only changes if the user runs
/// `xcode-select --switch`), so cache once and reuse.
static SDK_PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

fn sdk_path() -> Option<&'static str> {
    SDK_PATH
        .get_or_init(|| {
            Command::new("xcrun")
                .args(["--show-sdk-path"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
        })
        .as_deref()
}

pub fn link_dylib(tip_objs: &[PathBuf], stub_obj: &Path, out: &Path) -> Result<()> {
    if tip_objs.is_empty() {
        anyhow::bail!("link_dylib called with no tip objects");
    }

    // Apple's classic ld accepts the same args cc would have
    // passed it. The only translation needed vs `cc -dynamiclib`:
    //
    //  - cc passes `-syslibroot <SDK>` automatically; ld doesn't,
    //    so we resolve `xcrun --show-sdk-path` once and feed it in.
    //  - cc passes `-platform_version macos <min> <sdk>` based on
    //    its `-mmacosx-version-min` default; ld requires an
    //    explicit `-platform_version` triplet or it warns "no
    //    platform load command found".
    //  - `-Wl,-X` prefixes drop — those are cc syntax for "pass
    //    through to ld"; ld already IS the linker.
    let sdk = sdk_path().context(
        "xcrun --show-sdk-path failed; iOS / macOS SDK not on path. \
         Install Xcode or set DEVELOPER_DIR.",
    )?;

    let mut cmd = Command::new("ld");
    cmd.args([
        "-dylib",
        "-arch",
        "arm64",
        // Platform triplet: macos / min OS version / SDK version.
        // ld is picky here; missing → "no platform load command".
        // Hardcoding 11.0 keeps the dylib loadable on every macOS
        // version Rust supports.
        "-platform_version",
        "macos",
        "11.0",
        "11.0",
        "-syslibroot",
        sdk,
        // No default libs — the stub satisfies framework refs;
        // libSystem covers libc.
        "-lSystem",
        // `dynamic_lookup` is deprecated upstream but still works.
        // We pass it without the `-Wl,` prefix (that's cc syntax).
        "-undefined",
        "dynamic_lookup",
        // Suppress "no -no_warn_inits …" cosmetic warning.
        "-no_warn_inits",
    ]);
    for o in tip_objs {
        cmd.arg(o);
    }
    cmd.arg(stub_obj);
    cmd.arg("-o").arg(out);

    let output = cmd.output().with_context(|| "spawn ld")?;
    if !output.status.success() {
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &output.stderr);
        let _ = std::io::Write::write_all(&mut std::io::stderr(), &output.stdout);
        anyhow::bail!("ld exited with {}", output.status);
    }
    Ok(())
}

/// Link a SERVER patch ([`super::detour`]): the objects of every replayed
/// crate plus the stub, keeping only what the roots reach.
///
/// - `-dead_strip`: roots are the symbols the objects mark
///   `N_NO_DEAD_STRIP` (the functions the agent redirects) and the
///   exported `_main`. Code of the re-emitted crates that the binary had
///   dead-stripped is dropped here too, with the references it alone made
///   to functions the binary does not have.
/// - `-exported_symbol _main`: nothing else is exported, so no `pub fn`
///   of a crate becomes a root by being global. `_main` is the anchor the
///   agent `dlsym`s to learn where the patch was loaded.
/// - `-filelist`: a crate replays into up to 256 objects per crate, and
///   several crates' worth of paths overflow `ARG_MAX`.
pub fn link_patch(objs: &[PathBuf], stub_obj: &Path, out: &Path, filelist: &Path) -> Result<()> {
    if objs.is_empty() {
        anyhow::bail!("link_patch called with no objects");
    }
    let sdk = sdk_path().context(
        "xcrun --show-sdk-path failed; the macOS SDK is not on path. \
         Install Xcode or the Command Line Tools.",
    )?;
    let mut list = String::new();
    for o in objs.iter().map(PathBuf::as_path).chain(std::iter::once(stub_obj)) {
        list.push_str(&o.display().to_string());
        list.push('\n');
    }
    std::fs::write(filelist, list).with_context(|| format!("write {}", filelist.display()))?;
    let output = Command::new("ld")
        .args([
            "-dylib", "-arch", "arm64", "-platform_version", "macos", "11.0", "11.0", "-syslibroot", sdk,
            "-lSystem", "-undefined", "dynamic_lookup", "-no_warn_inits", "-dead_strip", "-exported_symbol",
            "_main",
        ])
        .arg("-filelist")
        .arg(filelist)
        .arg("-o")
        .arg(out)
        .output()
        .context("spawn ld")?;
    if !output.status.success() {
        anyhow::bail!(
            "ld exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).lines().take(12).collect::<Vec<_>>().join("\n")
        );
    }
    Ok(())
}
