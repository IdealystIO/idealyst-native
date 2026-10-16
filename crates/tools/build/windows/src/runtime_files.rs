//! Files an SDK needs beside the Windows `.exe` at runtime.
//!
//! Some Windows libraries come as a DLL the app loads at runtime rather than
//! code linked into it — Microsoft's WebView2 loader (`WebView2Loader.dll`)
//! is the case: its static form only links with the MSVC toolchain, and
//! Windows apps here are built with the GNU one. An SDK declares such a file
//! in its own `Cargo.toml`, naming the dependency package that carries it
//! (`{arch}` is the target's `x64` / `arm64` / `x86`):
//!
//! ```toml
//! [package.metadata.idealyst.windows]
//! runtime-files = [{ package = "webview2-com-sys", path = "{arch}/WebView2Loader.dll" }]
//! ```
//!
//! [`collect`] walks the wrapper's `cargo metadata` — the dependency closure
//! the app links, the same discovery as the iOS frameworks and the
//! capability registry — and resolves each entry to a file; the build
//! copies them next to the `.exe`, and `run --windows` ships them with it.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// The `{arch}` value for a target triple (`None` = this host).
pub(crate) fn arch_dir(triple: Option<&str>) -> &'static str {
    let arch = match triple {
        Some(t) => t.split('-').next().unwrap_or(""),
        None => std::env::consts::ARCH,
    };
    match arch {
        "aarch64" => "arm64",
        "x86" | "i686" | "i586" => "x86",
        _ => "x64",
    }
}

/// Every runtime file the wrapper's dependency closure declares, resolved.
pub(crate) fn collect(wrapper_manifest: &Path, arch: &str) -> Result<Vec<PathBuf>> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1"])
        .arg("--manifest-path")
        .arg(wrapper_manifest)
        .output()
        .with_context(|| format!("spawn cargo metadata --manifest-path {}", wrapper_manifest.display()))?;
    if !output.status.success() {
        bail!(
            "cargo metadata failed for {}: {}",
            wrapper_manifest.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let json: Value = serde_json::from_slice(&output.stdout).context("parse cargo metadata JSON")?;
    resolve(&json, arch)
}

/// The pure core of [`collect`]: resolve every declaration in `metadata`.
/// A declaration naming a package that is not in the graph is an error —
/// the SDK's manifest and its dependencies disagree, and shipping without
/// the file would fail only at runtime.
pub(crate) fn resolve(metadata: &Value, arch: &str) -> Result<Vec<PathBuf>> {
    let packages = metadata.get("packages").and_then(Value::as_array).cloned().unwrap_or_default();
    let dir_of = |name: &str| -> Option<PathBuf> {
        packages
            .iter()
            .find(|p| p.get("name").and_then(Value::as_str) == Some(name))
            .and_then(|p| p.get("manifest_path").and_then(Value::as_str))
            .and_then(|m| Path::new(m).parent().map(Path::to_path_buf))
    };
    let mut out = Vec::new();
    for pkg in &packages {
        let Some(files) = pkg.pointer("/metadata/idealyst/windows/runtime-files").and_then(Value::as_array) else {
            continue;
        };
        let declarer = pkg.get("name").and_then(Value::as_str).unwrap_or("?");
        for f in files {
            let (Some(from), Some(path)) = (f.get("package").and_then(Value::as_str), f.get("path").and_then(Value::as_str))
            else {
                bail!("{declarer}: each windows runtime-files entry needs `package` and `path`");
            };
            let Some(dir) = dir_of(from) else {
                bail!("{declarer} declares a runtime file from `{from}`, which is not in the dependency graph");
            };
            let file = dir.join(path.replace("{arch}", arch));
            if !out.contains(&file) {
                out.push(file);
            }
        }
    }
    Ok(out)
}

/// Copy `files` into `exe_dir` (skipping ones already identical there) and
/// return their new paths.
pub(crate) fn ship(files: &[PathBuf], exe_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut shipped = Vec::new();
    for src in files {
        let name = src.file_name().with_context(|| format!("runtime file {} has no name", src.display()))?;
        let dest = exe_dir.join(name);
        let bytes = std::fs::read(src).with_context(|| format!("read runtime file {}", src.display()))?;
        if std::fs::read(&dest).ok().as_deref() != Some(&bytes[..]) {
            std::fs::write(&dest, &bytes).with_context(|| format!("write {}", dest.display()))?;
        }
        shipped.push(dest);
    }
    Ok(shipped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn metadata() -> Value {
        json!({ "packages": [
            { "name": "webview", "manifest_path": "/w/webview/Cargo.toml",
              "metadata": { "idealyst": { "windows": { "runtime-files": [
                  { "package": "webview2-com-sys", "path": "{arch}/WebView2Loader.dll" } ] } } } },
            { "name": "webview2-com-sys", "manifest_path": "/reg/webview2-com-sys-0.39.1/Cargo.toml" },
            { "name": "other", "manifest_path": "/w/other/Cargo.toml", "metadata": null },
        ]})
    }

    /// An app using the webview SDK built without `WebView2Loader.dll`
    /// beside it: the SDK's declared runtime file was never shipped.
    #[test]
    fn regression_declared_runtime_files_resolve_to_the_carrying_package() {
        assert_eq!(
            resolve(&metadata(), "x64").unwrap(),
            [PathBuf::from("/reg/webview2-com-sys-0.39.1/x64/WebView2Loader.dll")]
        );
        assert_eq!(
            resolve(&metadata(), "arm64").unwrap(),
            [PathBuf::from("/reg/webview2-com-sys-0.39.1/arm64/WebView2Loader.dll")]
        );
    }

    #[test]
    fn a_declaration_from_a_package_outside_the_graph_is_an_error() {
        let mut m = metadata();
        m["packages"].as_array_mut().unwrap().remove(1);
        assert!(resolve(&m, "x64").unwrap_err().to_string().contains("webview2-com-sys"));
    }

    #[test]
    fn arch_dirs_follow_the_target() {
        assert_eq!(arch_dir(Some("x86_64-pc-windows-gnu")), "x64");
        assert_eq!(arch_dir(Some("aarch64-pc-windows-gnullvm")), "arm64");
        assert_eq!(arch_dir(Some("i686-pc-windows-gnu")), "x86");
    }

    #[test]
    fn ship_copies_beside_the_exe_and_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("Loader.dll");
        std::fs::write(&src, b"dll").unwrap();
        let exe_dir = tmp.path().join("bin");
        std::fs::create_dir(&exe_dir).unwrap();
        let shipped = ship(&[src.clone()], &exe_dir).unwrap();
        assert_eq!(shipped, [exe_dir.join("Loader.dll")]);
        assert_eq!(std::fs::read(&shipped[0]).unwrap(), b"dll");
        assert_eq!(ship(&[src], &exe_dir).unwrap(), shipped);
    }
}
