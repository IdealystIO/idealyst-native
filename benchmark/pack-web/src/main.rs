//! `bench-pack-web` — build one of the benchmark's wasm crates (a
//! `cdylib`) into `<crate>/pkg/`, the layout `wasm-pack build --target
//! web --release` used to produce.
//!
//! ```text
//! cargo run -q --release -p bench-pack-web -- benchmark/idealyst-native/wasm
//! # a profiling build into its own out-dir; args after `--` go to cargo:
//! cargo run -q --release -p bench-pack-web -- benchmark/idealyst-native/wasm \
//!     --out-dir benchmark/idealyst-native/wasm/pkg-prof -- --features debug-stats
//! ```
//!
//! Why not wasm-pack: backend-web's bindings are web-glue
//! (docs/proposals/own-web-bindings.md). Its JS rides inside the linked
//! module and the page needs it back as `pkg/__idealyst_glue.js`, which
//! only the framework's own build pass writes. wasm-pack still "succeeds"
//! on such a crate, but its output imports a `__idealyst_glue.js` that does
//! not exist: the page 404s on it, the module graph never loads, and the
//! runner times out with no error from the variant.
//!
//! Steps, matching wasm-pack's release build plus the hybrid pass
//! `idealyst build --web` runs (`build_web::own_glue`):
//!
//! 1. `cargo build --lib --release --target wasm32-unknown-unknown`;
//! 2. `own_glue::hybrid_extract` — take the glue out of the linked module
//!    (and repoint web-glue's own exports past LLD's constructor wrapper);
//! 3. `wasm-bindgen --target web` over the stripped module;
//! 4. `own_glue::write_hybrid_glue_file` — `pkg/__idealyst_glue.js`;
//! 5. `wasm-opt` with the crate's own
//!    `[package.metadata.wasm-pack.profile.release] wasm-opt` flags, so the
//!    optimisation level is exactly what wasm-pack applied before.
//!
//! A crate with no glue passes through steps 2 and 4 unchanged.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, ensure, Context, Result};
use build_web::own_glue;
use serde_json::Value;

const TARGET: &str = "wasm32-unknown-unknown";

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let crate_dir =
        PathBuf::from(args.next().context(
            "usage: bench-pack-web <crate-dir> [--out-dir <dir>] [-- <cargo build args>]",
        )?);
    let mut out_dir = crate_dir.join("pkg");
    let mut cargo_args = Vec::new();
    while let Some(a) = args.next() {
        match a.to_str() {
            Some("--out-dir") => {
                out_dir = PathBuf::from(args.next().context("--out-dir needs a value")?)
            }
            Some("--") => cargo_args.extend(args.by_ref()),
            _ => bail!("unknown argument {a:?}"),
        }
    }
    let crate_dir = crate_dir
        .canonicalize()
        .with_context(|| format!("no crate at {}", crate_dir.display()))?;

    let package = package_metadata(&crate_dir)?;
    let name = package["name"]
        .as_str()
        .context("package has no name")?
        .to_owned();
    let opt_flags = wasm_opt_flags(&package);

    let linked = cargo_build(&crate_dir, &name, &cargo_args)?;
    let lib_name = linked
        .file_stem()
        .and_then(|s| s.to_str())
        .context("linked wasm has no file name")?
        .to_owned();

    let (input, glue) = own_glue::hybrid_extract(&linked).context("web-glue extraction")?;

    if out_dir.exists() {
        fs::remove_dir_all(&out_dir).with_context(|| format!("clear {}", out_dir.display()))?;
    }
    fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    run(Command::new("wasm-bindgen")
        .args(["--target", "web", "--out-name", &lib_name, "--out-dir"])
        .arg(&out_dir)
        .arg(&input))
    .context("wasm-bindgen — is wasm-bindgen-cli installed at the Cargo.lock version?")?;
    own_glue::write_hybrid_glue_file(&out_dir, &glue, &lib_name)
        .context("write pkg/__idealyst_glue.js")?;

    let wasm = out_dir.join(format!("{lib_name}_bg.wasm"));
    let tmp = wasm.with_extension("wasm.opt");
    run(Command::new("wasm-opt")
        .args(&opt_flags)
        .arg("-o")
        .arg(&tmp)
        .arg(&wasm))
    .context("wasm-opt — is binaryen installed?")?;
    fs::rename(&tmp, &wasm)?;

    eprintln!(
        "[bench-pack-web] {name} → {} (glue imports: {}, wasm-opt {})",
        out_dir.display(),
        glue.imports.len(),
        opt_flags.join(" "),
    );
    Ok(())
}

/// The `cargo metadata` entry for the package whose manifest is in
/// `crate_dir`.
fn package_metadata(crate_dir: &Path) -> Result<Value> {
    let out = Command::new("cargo")
        .current_dir(crate_dir)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .stderr(Stdio::inherit())
        .output()
        .context("spawn cargo metadata")?;
    ensure!(
        out.status.success(),
        "cargo metadata failed: {}",
        out.status
    );
    let meta: Value = serde_json::from_slice(&out.stdout)?;
    let manifest = crate_dir.join("Cargo.toml");
    meta["packages"]
        .as_array()
        .context("cargo metadata: no packages")?
        .iter()
        .find(|p| p["manifest_path"].as_str().map(Path::new) == Some(manifest.as_path()))
        .cloned()
        .with_context(|| format!("{} is not a workspace member", manifest.display()))
}

/// wasm-pack's release `wasm-opt` flags from the crate's metadata, or
/// wasm-pack's own default (`-O`) when it declares none.
fn wasm_opt_flags(package: &Value) -> Vec<String> {
    package["metadata"]["wasm-pack"]["profile"]["release"]["wasm-opt"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_else(|| vec!["-O".to_owned()])
}

/// Build the crate's cdylib and return the linked `.wasm` cargo reports.
fn cargo_build(crate_dir: &Path, package: &str, extra: &[std::ffi::OsString]) -> Result<PathBuf> {
    let mut child = Command::new("cargo")
        .current_dir(crate_dir)
        .args([
            "build",
            "--lib",
            "--release",
            "--target",
            TARGET,
            "-p",
            package,
        ])
        .args(["--message-format", "json-render-diagnostics"])
        .args(extra)
        .stdout(Stdio::piped())
        .spawn()
        .context("spawn cargo build")?;
    let manifest = crate_dir.join("Cargo.toml");
    let mut wasm = None;
    for line in BufReader::new(child.stdout.take().unwrap()).lines() {
        let msg: Value = match serde_json::from_str(&line?) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if msg["reason"] == "compiler-artifact" && msg["target"]["name"].is_string() {
            let is_cdylib = msg["target"]["kind"]
                .as_array()
                .is_some_and(|k| k.iter().any(|k| k == "cdylib"));
            let ours = msg["manifest_path"].as_str().map(Path::new) == Some(manifest.as_path());
            if is_cdylib && ours {
                wasm = msg["filenames"]
                    .as_array()
                    .and_then(|f| {
                        f.iter()
                            .filter_map(Value::as_str)
                            .find(|f| f.ends_with(".wasm"))
                    })
                    .map(PathBuf::from);
            }
        }
    }
    let status = child.wait()?;
    ensure!(
        status.success(),
        "cargo build of {package} failed: {status}"
    );
    wasm.with_context(|| format!("cargo built no .wasm for {package}"))
}

fn run(cmd: &mut Command) -> Result<()> {
    let status = cmd.status()?;
    ensure!(status.success(), "{:?} failed: {status}", cmd.get_program());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The measured workload's optimisation level must be the one wasm-pack
    // applied: same flags, from the same table.
    #[test]
    fn wasm_opt_flags_come_from_the_wasm_pack_release_profile() {
        let pkg = serde_json::json!({ "metadata": { "wasm-pack": { "profile": { "release": {
            "wasm-opt": ["-Oz", "--strip-debug"]
        }}}}});
        assert_eq!(wasm_opt_flags(&pkg), ["-Oz", "--strip-debug"]);
    }

    #[test]
    fn wasm_opt_flags_default_to_wasm_packs_own_default() {
        assert_eq!(
            wasm_opt_flags(&serde_json::json!({ "metadata": null })),
            ["-O"]
        );
    }
}
