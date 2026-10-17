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
//! A DLL the SDK builds from source — a COM server another process loads,
//! say — is declared as an **artifact** instead: a dependency package whose
//! `cdylib` the CLI builds for the app's target with the given features
//! (`build_ios::sdk_products`) and ships as `<lib name>.dll`:
//!
//! ```toml
//! [[package.metadata.idealyst.windows.artifacts]]
//! package = "my-sdk-com-server"
//! kind = "cdylib"
//! features = ["server"]   # optional
//! ```
//!
//! [`metadata`] reads the wrapper's `cargo metadata` — the dependency closure
//! the app links, the same discovery as the iOS frameworks and the
//! capability registry — and resolves each entry; the build places them
//! next to the `.exe`, and `run --windows` ships them with it.

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

/// The wrapper's `cargo metadata`.
pub(crate) fn metadata(wrapper_manifest: &Path) -> Result<Value> {
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
    serde_json::from_slice(&output.stdout).context("parse cargo metadata JSON")
}

/// Resolve every `runtime-files` declaration in `metadata`.
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

/// One `artifacts` declaration, resolved against the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ArtifactPlan {
    /// The package that declared it (for errors).
    pub declarer: String,
    pub package: String,
    /// The package's source directory.
    pub manifest_dir: PathBuf,
    /// Its library target's name — the DLL's file stem.
    pub lib_name: String,
    pub features: Vec<String>,
}

impl ArtifactPlan {
    pub fn dll_name(&self) -> String {
        format!("{}.dll", self.lib_name)
    }
}

/// Resolve every `artifacts` declaration in `metadata`. Only `cdylib` is a
/// Windows artifact kind; the named package must be in the graph and its
/// library must list `cdylib` among its crate types.
pub(crate) fn resolve_artifacts(metadata: &Value) -> Result<Vec<ArtifactPlan>> {
    let packages = metadata.get("packages").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out: Vec<ArtifactPlan> = Vec::new();
    for pkg in &packages {
        let Some(decls) = pkg.pointer("/metadata/idealyst/windows/artifacts").and_then(Value::as_array) else {
            continue;
        };
        let declarer = pkg.get("name").and_then(Value::as_str).unwrap_or("?");
        for d in decls {
            let Some(name) = d.get("package").and_then(Value::as_str) else {
                bail!("{declarer}: each windows artifacts entry needs `package`");
            };
            match d.get("kind").and_then(Value::as_str) {
                Some("cdylib") => {}
                other => bail!(
                    "{declarer}: windows artifact `{name}` has kind {other:?}; the only Windows artifact kind is \"cdylib\""
                ),
            }
            let features = string_list(d.get("features"))
                .with_context(|| format!("{declarer}: windows artifact `{name}` `features`"))?;
            let Some(target_pkg) = packages.iter().find(|p| p.get("name").and_then(Value::as_str) == Some(name)) else {
                bail!("{declarer} declares a windows artifact from `{name}`, which is not in the dependency graph");
            };
            let manifest_dir = target_pkg
                .get("manifest_path")
                .and_then(Value::as_str)
                .and_then(|m| Path::new(m).parent().map(Path::to_path_buf))
                .with_context(|| format!("{name}: cargo metadata has no manifest_path"))?;
            let Some(lib_name) = cdylib_name(target_pkg) else {
                bail!("{declarer} declares a cdylib artifact from `{name}`, whose library is not a cdylib (add `crate-type = [\"rlib\", \"cdylib\"]`)");
            };
            let plan = ArtifactPlan { declarer: declarer.to_string(), package: name.to_string(), manifest_dir, lib_name, features };
            if !out.iter().any(|p| p.package == plan.package && p.features == plan.features) {
                out.push(plan);
            }
        }
    }
    Ok(out)
}

/// The name of `pkg`'s library target when it builds a `cdylib`.
fn cdylib_name(pkg: &Value) -> Option<String> {
    pkg.get("targets")?.as_array()?.iter().find_map(|t| {
        let kinds = t.get("crate_types").or_else(|| t.get("kind"))?.as_array()?;
        kinds
            .iter()
            .any(|k| k.as_str() == Some("cdylib"))
            .then(|| t.get("name").and_then(Value::as_str).map(str::to_string))
            .flatten()
    })
}

/// An optional array of strings.
fn string_list(v: Option<&Value>) -> Result<Vec<String>> {
    match v {
        None => Ok(Vec::new()),
        Some(Value::Array(items)) => items
            .iter()
            .map(|i| i.as_str().map(str::to_string).context("must be an array of strings"))
            .collect(),
        Some(_) => bail!("must be an array of strings"),
    }
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

    fn artifact_metadata() -> Value {
        json!({ "packages": [
            { "name": "cam-sdk", "manifest_path": "/w/cam-sdk/Cargo.toml",
              "metadata": { "idealyst": { "windows": { "artifacts": [
                  { "package": "cam-source", "kind": "cdylib", "features": ["source"] } ] } } } },
            { "name": "cam-source", "manifest_path": "/reg/cam-source-1.0.0/Cargo.toml",
              "targets": [ { "name": "cam_source", "kind": ["cdylib", "rlib"], "crate_types": ["cdylib", "rlib"] } ] },
        ]})
    }

    #[test]
    fn artifact_declarations_resolve_to_a_build_plan() {
        assert_eq!(
            resolve_artifacts(&artifact_metadata()).unwrap(),
            [ArtifactPlan {
                declarer: "cam-sdk".into(),
                package: "cam-source".into(),
                manifest_dir: PathBuf::from("/reg/cam-source-1.0.0"),
                lib_name: "cam_source".into(),
                features: vec!["source".into()],
            }]
        );
        assert_eq!(resolve_artifacts(&artifact_metadata()).unwrap()[0].dll_name(), "cam_source.dll");
        // Runtime-files resolution ignores artifact tables.
        assert!(resolve(&artifact_metadata(), "x64").unwrap().is_empty());
    }

    #[test]
    fn artifact_features_are_optional() {
        let mut m = artifact_metadata();
        m["packages"][0]["metadata"]["idealyst"]["windows"]["artifacts"][0]
            .as_object_mut()
            .unwrap()
            .remove("features");
        assert!(resolve_artifacts(&m).unwrap()[0].features.is_empty());
    }

    #[test]
    fn an_artifact_from_a_package_outside_the_graph_is_an_error() {
        let mut m = artifact_metadata();
        m["packages"].as_array_mut().unwrap().remove(1);
        let err = resolve_artifacts(&m).unwrap_err().to_string();
        assert!(err.contains("cam-source") && err.contains("not in the dependency graph"), "{err}");
    }

    #[test]
    fn an_artifact_whose_library_is_not_a_cdylib_is_an_error() {
        let mut m = artifact_metadata();
        m["packages"][1]["targets"][0]["crate_types"] = json!(["rlib"]);
        m["packages"][1]["targets"][0]["kind"] = json!(["lib"]);
        assert!(resolve_artifacts(&m).unwrap_err().to_string().contains("not a cdylib"));
        let mut m = artifact_metadata();
        m["packages"][0]["metadata"]["idealyst"]["windows"]["artifacts"][0]["kind"] = json!("bin");
        assert!(resolve_artifacts(&m).unwrap_err().to_string().contains("\"cdylib\""));
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
