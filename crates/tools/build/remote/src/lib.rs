//! Release builds of remote-component bundles: `idealyst build --remote`.
//!
//! An app declares its bundles in its `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.idealyst.remote]
//! bundles = [
//!   { name = "shop", package = "shop-screens" },
//!   { name = "settings", package = "my-app" },   # the app's own library
//! ]
//! ```
//!
//! Each bundle is a library crate in the app's workspace, compiled to wasm
//! under its OWN crate name: the `#[component(remote)]`s in it are exported
//! by `module_path::Name`, which is what the app's stubs (the same crate,
//! compiled natively) ask for. So nothing needs renaming, and the crate
//! doesn't declare `crate-type = ["cdylib"]`: the build asks for one
//! (`cargo rustc --crate-type cdylib`).
//!
//! For each bundle the build:
//! 1. compiles it for `wasm32-unknown-unknown` with `--cfg idealyst_stream_guest`
//!    (a BUNDLE build: `#[host_fn]`s become stubs, app components imports);
//! 2. checks it imports only what the loader provides — a crate pulling in
//!    web bindings (`wasm-bindgen`) or native code compiles, then fails to
//!    load; here the build fails instead, naming the imports;
//! 3. reads the codec version the bundle was built with (the vocabulary's
//!    `idealyst.codec` section), which also proves the crate is a bundle;
//! 4. stamps its metadata (`remote_bundle::Metadata`) and, given a key,
//!    signs it;
//! 5. writes `<out>/<name>.wasm` and `<out>/<name>.json` (name, package,
//!    version, codec, size, SHA-256, signing key id).
//!
//! A crate's native-only dependencies go under
//! `[target.'cfg(not(idealyst_stream_guest))'.dependencies]`, so the bundle
//! build doesn't compile them.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use remote_bundle::{Metadata, SigningKey};
use serde::{Deserialize, Serialize};

/// The import modules the loader (`remote-host`) defines. Anything else is
/// a crate that can't run in a bundle.
pub const LOADER_IMPORT_MODULES: &[&str] = &["idealyst_kernel", "idealyst_ui", "idealyst_host_fn"];

/// The custom section the vocabulary stamps with its codec version.
pub const CODEC_SECTION: &str = "idealyst.codec";

/// The environment variable a signing key may come from (its hex form),
/// for CI.
pub const SIGNING_KEY_ENV: &str = "IDEALYST_REMOTE_SIGNING_KEY";

/// The flags a bundle is compiled with, `CARGO_ENCODED_RUSTFLAGS`-encoded
/// (0x1f-separated). The same as the dev builds' (`stream-spike`'s
/// `guest_build_command`):
/// - a 64 KB stack: rustc's 1 MB default makes the initial memory 17 pages,
///   which the interpreter zeroes at every load (~0.9 ms of ~1.06);
/// - `--cfg idealyst_stream_guest`: a bundle build. A cfg rather than a cargo
///   feature, so it can never reach an app build through unification;
/// - `-Aunused`: an app component's body is compiled out of a bundle, so the
///   helpers only it used read as unused here.
pub const BUNDLE_RUSTFLAGS: &str = "-Clink-arg=-zstack-size=65536\x1f--cfg=idealyst_stream_guest\x1f-Aunused";

/// One bundle the app declares.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BundleSpec {
    /// The bundle's name: its file name, and what the app calls it.
    pub name: String,
    /// The workspace crate whose library it is.
    pub package: String,
}

/// The bundles declared in `manifest` (a `Cargo.toml`'s text).
pub fn bundle_specs(manifest: &str) -> Result<Vec<BundleSpec>> {
    #[derive(Deserialize)]
    struct Remote {
        #[serde(default)]
        bundles: Vec<BundleSpec>,
    }
    let doc: toml::Value = toml::from_str(manifest).context("parse Cargo.toml")?;
    let Some(remote) = doc.get("package").and_then(|p| p.get("metadata")).and_then(|m| m.get("idealyst")).and_then(|i| i.get("remote"))
    else {
        return Ok(Vec::new());
    };
    let remote: Remote = remote.clone().try_into().context("[package.metadata.idealyst.remote]")?;
    let mut seen = std::collections::HashSet::new();
    for b in &remote.bundles {
        if b.name.is_empty() || !b.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            bail!("bundle name {:?}: use letters, digits, `-` and `_` (it becomes a file name)", b.name);
        }
        if !seen.insert(&b.name) {
            bail!("two bundles are named {:?}", b.name);
        }
    }
    Ok(remote.bundles)
}

/// What a built bundle is, as written next to it (`<name>.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    pub package: String,
    pub version: String,
    pub codec: u32,
    /// The bundle's file name, beside this manifest.
    pub file: String,
    pub size: u64,
    pub sha256: String,
    /// The id of the key that signed it, if it is signed.
    pub signed_by: Option<String>,
}

/// A bundle the build wrote.
#[derive(Debug)]
pub struct Built {
    pub wasm: PathBuf,
    pub manifest: Manifest,
}

/// How to build.
#[derive(Default)]
pub struct Options {
    /// Where the bundles go; `<target>/idealyst/remote` by default.
    pub out_dir: Option<PathBuf>,
    /// Sign every bundle with this key.
    pub sign: Option<SigningKey>,
    /// Build only these bundles (by name); all when empty.
    pub only: Vec<String>,
}

/// A workspace package, as `cargo metadata` describes it.
struct Package {
    version: String,
    /// The library's crate name (`my_screens`), what the wasm is named.
    lib: Option<String>,
}

struct Workspace {
    target_dir: PathBuf,
    packages: std::collections::HashMap<String, Package>,
}

fn workspace(app_dir: &Path) -> Result<Workspace> {
    let out = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(app_dir)
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .output()
        .context("run `cargo metadata`")?;
    if !out.status.success() {
        bail!("`cargo metadata` failed:\n{}", String::from_utf8_lossy(&out.stderr));
    }
    let meta: serde_json::Value = serde_json::from_slice(&out.stdout).context("parse `cargo metadata`")?;
    let target_dir = PathBuf::from(meta["target_directory"].as_str().ok_or_else(|| anyhow!("no target_directory"))?);
    let mut packages = std::collections::HashMap::new();
    for p in meta["packages"].as_array().into_iter().flatten() {
        let lib = p["targets"].as_array().into_iter().flatten().find_map(|t| {
            let kinds = t["kind"].as_array()?;
            kinds.iter().any(|k| matches!(k.as_str(), Some("lib" | "rlib" | "cdylib"))).then(|| t["name"].as_str().map(|n| n.replace('-', "_")))?
        });
        packages.insert(
            p["name"].as_str().unwrap_or_default().to_string(),
            Package { version: p["version"].as_str().unwrap_or_default().to_string(), lib },
        );
    }
    Ok(Workspace { target_dir, packages })
}

/// Fails, with how to fix it, when the wasm32 target isn't installed.
fn require_wasm32() -> Result<()> {
    let rustc = std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into());
    let dir = Command::new(&rustc)
        .args(["--print", "target-libdir", "--target", "wasm32-unknown-unknown"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()));
    if dir.is_some_and(|d| std::fs::read_dir(d).is_ok_and(|mut e| e.next().is_some())) {
        Ok(())
    } else {
        bail!("remote bundles are wasm: install the target with `rustup target add wasm32-unknown-unknown`")
    }
}

/// The import modules `wasm` uses that the loader doesn't define, with an
/// example import from each.
pub fn foreign_imports(wasm: &[u8]) -> Result<Vec<(String, String)>> {
    let mut foreign: Vec<(String, String)> = Vec::new();
    for payload in wasmparser::Parser::new(0).parse_all(wasm) {
        if let wasmparser::Payload::ImportSection(reader) = payload.context("read the bundle's imports")? {
            for import in reader {
                let import = import.context("read an import")?;
                if !LOADER_IMPORT_MODULES.contains(&import.module) && !foreign.iter().any(|(m, _)| m == import.module) {
                    foreign.push((import.module.to_string(), import.name.to_string()));
                }
            }
        }
    }
    Ok(foreign)
}

/// The codec version stamped in `wasm`'s `idealyst.codec` section.
pub fn codec_version(wasm: &[u8]) -> Result<Option<u32>> {
    for s in remote_bundle::sections(wasm)? {
        if s.custom_name.as_deref() == Some(CODEC_SECTION) {
            let bytes: [u8; 4] = wasm[s.payload].try_into().map_err(|_| anyhow!("the codec section isn't 4 bytes"))?;
            return Ok(Some(u32::from_le_bytes(bytes)));
        }
    }
    Ok(None)
}

/// Stamp `wasm` (a freshly compiled bundle of `spec`) as a release: check
/// it, add its metadata, sign it if asked. The pure half of [`build`].
pub fn finish(wasm: &[u8], spec: &BundleSpec, version: &str, sign: Option<&SigningKey>) -> Result<(Vec<u8>, Manifest)> {
    let foreign = foreign_imports(wasm)?;
    if !foreign.is_empty() {
        let list = foreign.iter().map(|(m, n)| format!("`{m}` (e.g. `{n}`)")).collect::<Vec<_>>().join(", ");
        let hint = if foreign.iter().any(|(m, _)| m.contains("wbindgen") || m == "wbg") {
            " Those are wasm-bindgen's: a crate the bundle compiles uses web bindings \
             (`cargo tree -i wasm-bindgen --target wasm32-unknown-unknown` shows which). Move it under \
             `[target.'cfg(not(idealyst_stream_guest))'.dependencies]`."
        } else {
            " A crate the bundle compiles calls code the loader doesn't provide (C, or a platform API). \
             Move it under `[target.'cfg(not(idealyst_stream_guest))'.dependencies]`, or call it from \
             the app through a `#[host_fn]`."
        };
        bail!("bundle `{}` ({}) imports what the loader doesn't provide: {list}.{hint}", spec.name, spec.package);
    }
    let codec = codec_version(wasm)?.ok_or_else(|| {
        anyhow!(
            "`{}` didn't build as a remote-component bundle: it has no `{CODEC_SECTION}` section, which \
             runtime-vocabulary's bundle half adds. Does it depend on runtime-vocabulary with the `remote` \
             feature, and define a `#[component(remote)]`?",
            spec.package
        )
    })?;
    let meta = Metadata { name: spec.name.clone(), package: spec.package.clone(), version: version.to_string(), codec };
    let mut out = remote_bundle::with_metadata(wasm, &meta)?;
    if let Some(key) = sign {
        out = remote_bundle::sign(&out, key)?;
    }
    let manifest = Manifest {
        name: spec.name.clone(),
        package: spec.package.clone(),
        version: version.to_string(),
        codec,
        file: format!("{}.wasm", spec.name),
        size: out.len() as u64,
        sha256: remote_bundle::content_hash(&out),
        signed_by: sign.map(|k| k.public().id().to_string()),
    };
    Ok((out, manifest))
}

/// The `cargo` invocation that compiles `package`'s library as a bundle
/// into `target_dir`.
pub fn bundle_command(app_dir: &Path, package: &str, target_dir: &Path) -> Command {
    let mut cmd = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(app_dir)
        .args(["rustc", "-p", package, "--lib", "--release", "--target", "wasm32-unknown-unknown", "--crate-type", "cdylib"])
        .arg("--target-dir")
        .arg(target_dir)
        .env("CARGO_ENCODED_RUSTFLAGS", BUNDLE_RUSTFLAGS)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_BUILD_TARGET")
        .env_remove("CARGO_TARGET_DIR");
    cmd
}

/// Build the bundles `app_dir`'s `Cargo.toml` declares.
pub fn build(app_dir: &Path, options: &Options) -> Result<Vec<Built>> {
    let manifest = std::fs::read_to_string(app_dir.join("Cargo.toml")).with_context(|| format!("read {}", app_dir.join("Cargo.toml").display()))?;
    let mut specs = bundle_specs(&manifest)?;
    if specs.is_empty() {
        bail!(
            "no remote bundles declared: add them to Cargo.toml\n\n  [package.metadata.idealyst.remote]\n  \
             bundles = [{{ name = \"screens\", package = \"my-screens\" }}]"
        );
    }
    if !options.only.is_empty() {
        for name in &options.only {
            if !specs.iter().any(|s| &s.name == name) {
                bail!("no bundle named {name:?} (declared: {})", specs.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", "));
            }
        }
        specs.retain(|s| options.only.contains(&s.name));
    }
    require_wasm32()?;
    let ws = workspace(app_dir)?;
    let build_dir = ws.target_dir.join("idealyst").join("remote-build");
    let out_dir = options.out_dir.clone().unwrap_or_else(|| ws.target_dir.join("idealyst").join("remote"));
    std::fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;

    let mut built = Vec::new();
    for spec in &specs {
        let pkg = ws.packages.get(&spec.package).ok_or_else(|| {
            anyhow!("bundle `{}`: no package `{}` in this workspace (a bundle is a workspace crate)", spec.name, spec.package)
        })?;
        let lib = pkg.lib.as_ref().ok_or_else(|| anyhow!("bundle `{}`: `{}` has no library target", spec.name, spec.package))?;
        let status = bundle_command(app_dir, &spec.package, &build_dir)
            .status()
            .with_context(|| format!("run cargo for bundle `{}`", spec.name))?;
        if !status.success() {
            bail!("bundle `{}` ({}) failed to compile for wasm32", spec.name, spec.package);
        }
        let compiled = build_dir.join("wasm32-unknown-unknown/release").join(format!("{lib}.wasm"));
        let wasm = std::fs::read(&compiled).with_context(|| format!("read {}", compiled.display()))?;
        let (release, manifest) = finish(&wasm, spec, &pkg.version, options.sign.as_ref())?;
        let path = out_dir.join(&manifest.file);
        std::fs::write(&path, &release).with_context(|| format!("write {}", path.display()))?;
        std::fs::write(out_dir.join(format!("{}.json", spec.name)), serde_json::to_vec_pretty(&manifest)?)?;
        built.push(Built { wasm: path, manifest });
    }
    Ok(built)
}

/// A signing key from a key file (its hex), or `None` without a file and
/// without [`SIGNING_KEY_ENV`].
pub fn signing_key(file: Option<&Path>) -> Result<Option<SigningKey>> {
    let text = match file {
        Some(f) => std::fs::read_to_string(f).with_context(|| format!("read signing key {}", f.display()))?,
        None => match std::env::var(SIGNING_KEY_ENV) {
            Ok(v) => v,
            Err(_) => return Ok(None),
        },
    };
    SigningKey::from_hex(&text).map(Some).map_err(|e| anyhow!("signing key: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundles_are_read_from_the_manifest() {
        let toml = r#"
            [package]
            name = "app"
            [package.metadata.idealyst.remote]
            bundles = [{ name = "shop", package = "shop-screens" }, { name = "settings", package = "app" }]
        "#;
        assert_eq!(
            bundle_specs(toml).unwrap(),
            [
                BundleSpec { name: "shop".into(), package: "shop-screens".into() },
                BundleSpec { name: "settings".into(), package: "app".into() }
            ]
        );
        assert!(bundle_specs("[package]\nname = \"app\"").unwrap().is_empty());
        let dup = r#"[package.metadata.idealyst.remote]
            bundles = [{ name = "a", package = "x" }, { name = "a", package = "y" }]"#;
        assert!(bundle_specs(dup).unwrap_err().to_string().contains("two bundles"));
        let bad = r#"[package.metadata.idealyst.remote]
            bundles = [{ name = "../a", package = "x" }]"#;
        assert!(bundle_specs(bad).unwrap_err().to_string().contains("file name"));
    }

    /// The bundle is compiled under its own name, as a bundle (the guest
    /// cfg), as a cdylib, in its own target dir.
    #[test]
    fn the_cargo_invocation_builds_a_cdylib_bundle() {
        let cmd = bundle_command(Path::new("/app"), "shop-screens", Path::new("/t"));
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            ["rustc", "-p", "shop-screens", "--lib", "--release", "--target", "wasm32-unknown-unknown", "--crate-type", "cdylib", "--target-dir", "/t"]
        );
        let flags = cmd.get_envs().find(|(k, _)| *k == "CARGO_ENCODED_RUSTFLAGS").and_then(|(_, v)| v).unwrap();
        assert!(flags.to_string_lossy().contains("--cfg=idealyst_stream_guest"));
    }

    /// A module with one import from `module`, and optionally the codec
    /// section.
    fn module(import_module: &str, codec: Option<u32>) -> Vec<u8> {
        let mut m = b"\0asm\x01\0\0\0".to_vec();
        m.extend_from_slice(&[1, 4, 1, 0x60, 0, 0]); // type () -> ()
        let mut imp = vec![1u8, import_module.len() as u8];
        imp.extend_from_slice(import_module.as_bytes());
        imp.extend_from_slice(&[1, b'f', 0, 0]); // name "f", func, type 0
        m.push(2);
        m.push(imp.len() as u8);
        m.extend_from_slice(&imp);
        if let Some(c) = codec {
            let mut body = vec![CODEC_SECTION.len() as u8];
            body.extend_from_slice(CODEC_SECTION.as_bytes());
            body.extend_from_slice(&c.to_le_bytes());
            m.push(0);
            m.push(body.len() as u8);
            m.extend_from_slice(&body);
        }
        m
    }

    fn spec() -> BundleSpec {
        BundleSpec { name: "shop".into(), package: "shop-screens".into() }
    }

    #[test]
    fn a_bundle_importing_web_bindings_is_refused_naming_them() {
        let err = finish(&module("__wbindgen_placeholder__", Some(2)), &spec(), "1.0.0", None).unwrap_err().to_string();
        assert!(err.contains("__wbindgen_placeholder__") && err.contains("wasm-bindgen"), "{err}");
        let err = finish(&module("env", Some(2)), &spec(), "1.0.0", None).unwrap_err().to_string();
        assert!(err.contains("`env`") && err.contains("#[host_fn]"), "{err}");
    }

    #[test]
    fn a_crate_that_isnt_a_bundle_is_refused() {
        let err = finish(&module("idealyst_kernel", None), &spec(), "1.0.0", None).unwrap_err().to_string();
        assert!(err.contains("didn't build as a remote-component bundle"), "{err}");
    }

    #[test]
    fn a_release_carries_its_metadata_and_signature() {
        let key = SigningKey::generate().unwrap();
        let (out, manifest) = finish(&module("idealyst_ui", Some(2)), &spec(), "1.4.0", Some(&key)).unwrap();
        assert_eq!(
            remote_bundle::metadata(&out).unwrap(),
            Some(Metadata { name: "shop".into(), package: "shop-screens".into(), version: "1.4.0".into(), codec: 2 })
        );
        assert_eq!(remote_bundle::verify(&out, &[key.public()]), Ok(key.public().id()));
        assert_eq!(manifest.sha256, remote_bundle::content_hash(&out));
        assert_eq!(manifest.size, out.len() as u64);
        assert_eq!(manifest.signed_by, Some(key.public().id().to_string()));
        assert_eq!(manifest.file, "shop.wasm");
        let (unsigned, m) = finish(&module("idealyst_ui", Some(2)), &spec(), "1.4.0", None).unwrap();
        assert_eq!(remote_bundle::signature(&unsigned).unwrap(), None);
        assert_eq!(m.signed_by, None);
    }

    #[test]
    fn a_signing_key_comes_from_a_file() {
        let key = SigningKey::generate().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("key");
        std::fs::write(&file, format!("{}\n", key.to_hex())).unwrap();
        assert_eq!(signing_key(Some(&file)).unwrap().unwrap().public(), key.public());
        std::fs::write(&file, "nope").unwrap();
        assert!(signing_key(Some(&file)).is_err());
    }
}
