//! Swift sources contributed by SDK crates.
//!
//! An Apple API with no Objective-C interface (StoreKit 2's `Product` and
//! `Transaction`, most new Swift-only frameworks) can't be called from Rust
//! through `objc2`. An SDK reaches it through a Swift file of its own that
//! exposes `@_cdecl` functions, which its Rust declares `extern "C"` and
//! calls. The SDK names the files in its `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.idealyst.ios]
//! runtime_swift = ["runtime/swift/StoreBridge.swift"]
//! frameworks = ["StoreKit"]
//! ```
//!
//! Paths are relative to the declaring package's directory, like Android's
//! `runtime_kotlin`. [`collect_runtime_swift`] walks the wrapper's `cargo
//! metadata`, whose packages are exactly the app's dependency closure, and
//! [`stage`] copies the files into `<swift dir>/sdk/`. Both builds compile
//! them into the app beside the generated `AppDelegate.swift`: the
//! simulator's `swiftc` call lists them, and the device project's Sources
//! phase references them. The Swift and the Rust staticlib link into one
//! executable, so the `@_cdecl` symbols resolve at link time.
//!
//! Files share one Swift module (the app's), so two SDKs must not ship the
//! same file name; that fails the build naming both.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// The folder under the Swift source dir the SDK files are staged into.
pub const SDK_SWIFT_DIR: &str = "sdk";

/// One SDK Swift file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwiftSource {
    /// The declaring crate, for diagnostics.
    pub package: String,
    pub source: PathBuf,
    /// Its file name, which is its name under [`SDK_SWIFT_DIR`].
    pub file_name: String,
}

/// Every `runtime_swift` entry in the wrapper's dependency closure. A
/// missing wrapper yields none.
pub fn collect_runtime_swift(wrapper_manifest: &Path) -> Result<Vec<SwiftSource>> {
    if !wrapper_manifest.is_file() {
        return Ok(Vec::new());
    }
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
            String::from_utf8_lossy(&output.stderr),
        );
    }
    let json: Value = serde_json::from_slice(&output.stdout).context("parse cargo metadata JSON")?;
    from_metadata(&json)
}

/// The pure core of [`collect_runtime_swift`], sorted by file name.
pub fn from_metadata(json: &Value) -> Result<Vec<SwiftSource>> {
    let mut by_name: BTreeMap<String, SwiftSource> = BTreeMap::new();
    let packages = json.get("packages").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    for pkg in &packages {
        let Some(entries) = pkg.pointer("/metadata/idealyst/ios/runtime_swift") else {
            continue;
        };
        let name = pkg.get("name").and_then(|v| v.as_str()).unwrap_or("<unknown>");
        let dir = pkg
            .get("manifest_path")
            .and_then(|v| v.as_str())
            .and_then(|p| Path::new(p).parent())
            .ok_or_else(|| anyhow::anyhow!("package {name} has no manifest_path"))?;
        let entries = entries
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("{name}: metadata.idealyst.ios.runtime_swift must be an array"))?;
        for e in entries {
            let rel = e
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{name}: metadata.idealyst.ios.runtime_swift entries must be strings"))?;
            let p = Path::new(rel);
            if p.is_absolute() || p.components().any(|c| matches!(c, Component::ParentDir)) {
                bail!("{name}: runtime_swift entry {rel:?} must be relative to the package and not use `..`");
            }
            let file_name = p
                .file_name()
                .and_then(|f| f.to_str())
                .filter(|f| f.ends_with(".swift"))
                .ok_or_else(|| anyhow::anyhow!("{name}: runtime_swift entry {rel:?} must name a .swift file"))?
                .to_string();
            let source = dir.join(rel);
            if !source.is_file() {
                bail!("{name} declares {} (via metadata.idealyst.ios.runtime_swift) but the file does not exist", source.display());
            }
            let entry = SwiftSource { package: name.to_string(), source, file_name: file_name.clone() };
            if let Some(other) = by_name.insert(file_name.clone(), entry) {
                bail!(
                    "{file_name} is declared by both {} and {name} (metadata.idealyst.ios.runtime_swift); \
                     SDK Swift files share the app's module, so their names must differ",
                    other.package
                );
            }
        }
    }
    Ok(by_name.into_values().collect())
}

/// Copy `sources` into `<swift_dir>/sdk/`, replacing what an earlier build
/// staged. Returns the staged paths, in order.
pub fn stage(sources: &[SwiftSource], swift_dir: &Path) -> Result<Vec<PathBuf>> {
    let dir = swift_dir.join(SDK_SWIFT_DIR);
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("clear {}", dir.display()))?;
    }
    if sources.is_empty() {
        return Ok(Vec::new());
    }
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    sources
        .iter()
        .map(|s| {
            let to = dir.join(&s.file_name);
            fs::copy(&s.source, &to)
                .with_context(|| format!("stage {} (from {}) → {}", s.source.display(), s.package, to.display()))?;
            Ok(to)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sdk(name: &str, dir: &Path, files: &[&str]) -> Value {
        for f in files {
            let p = dir.join(f);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, "import StoreKit\n").unwrap();
        }
        json!({
            "name": name,
            "manifest_path": dir.join("Cargo.toml").display().to_string(),
            "metadata": { "idealyst": { "ios": { "runtime_swift": files } } },
        })
    }

    #[test]
    fn sdk_swift_files_are_collected_and_staged() {
        let a = tempfile::tempdir().unwrap();
        let out = tempfile::tempdir().unwrap();
        let meta = json!({ "packages": [sdk("billing", a.path(), &["runtime/swift/StoreBridge.swift"])] });
        let sources = from_metadata(&meta).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].file_name, "StoreBridge.swift");
        let staged = stage(&sources, out.path()).unwrap();
        assert_eq!(staged, vec![out.path().join("sdk/StoreBridge.swift")]);
        assert!(staged[0].is_file());
        // Dropping the SDK drops its staged file.
        assert!(stage(&[], out.path()).unwrap().is_empty());
        assert!(!out.path().join("sdk").exists());
    }

    #[test]
    fn two_sdks_shipping_one_file_name_fail_naming_both() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let meta = json!({ "packages": [
            sdk("sdk-a", a.path(), &["swift/Bridge.swift"]),
            sdk("sdk-b", b.path(), &["swift/Bridge.swift"]),
        ]});
        let msg = format!("{:#}", from_metadata(&meta).unwrap_err());
        assert!(msg.contains("sdk-a") && msg.contains("sdk-b"), "{msg}");
    }

    #[test]
    fn entries_must_be_package_relative_swift_files() {
        let a = tempfile::tempdir().unwrap();
        for bad in ["../x.swift", "runtime/Bridge.m"] {
            let meta = json!({ "packages": [{
                "name": "bad", "manifest_path": a.path().join("Cargo.toml").display().to_string(),
                "metadata": { "idealyst": { "ios": { "runtime_swift": [bad] } } },
            }]});
            assert!(from_metadata(&meta).is_err(), "{bad} must be refused");
        }
    }
}
