//! Bundles an SDK needs inside the macOS `.app`.
//!
//! Some macOS integrations are separate executables the OS loads from a
//! bundle nested in the app: a CoreAudio `AudioServerPlugIn` (`.driver`), a
//! CoreMediaIO camera system extension (`.systemextension`). The SDK
//! declares each in its own `Cargo.toml`, naming a dependency package that
//! carries the code, and the CLI builds that package for the app's target
//! (`build_ios::sdk_products`) and wraps the result:
//!
//! ```toml
//! [package.metadata.idealyst.macos]
//! app-entitlements = ["com.apple.developer.system-extension.install"]
//!
//! [[package.metadata.idealyst.macos.bundles]]
//! package = "my-sdk-camera-extension"   # in the dependency graph
//! kind = "bin"                          # or "cdylib"
//! bin = "my-camera-extension"           # required for kind = "bin"
//! features = ["extension"]              # optional
//! bundle = "{{bundle_id}}.camera.systemextension"
//! place = "Contents/Library/SystemExtensions"
//! info-plist = "bundle/Info.plist"      # relative to the package's manifest dir
//! entitlements = "bundle/ext.entitlements"   # optional, same base
//! bundle-id = "{{bundle_id}}.camera"
//! env = { MY_SDK_VENDOR = "{{app_name}}" }   # optional, set while building
//! ```
//!
//! The result is `<App>.app/<place>/<bundle>/Contents/{Info.plist,
//! MacOS/<executable>}`, where the executable is the binary or the cdylib's
//! file. Signing (nested bundles first, each with its own entitlements; the
//! app last, with `app-entitlements` added) is `run-macos`'s.
//!
//! # Templates
//!
//! `bundle`, `bundle-id` and `env` values take `{{bundle_id}}` (the APP's
//! bundle id) and `{{app_name}}`. The `info-plist` and `entitlements` files
//! take `{{bundle_id}}` (the NESTED bundle's resolved id), `{{app_bundle_id}}`,
//! `{{app_name}}`, `{{version}}`, `{{executable}}`, `{{team_id}}` and
//! `{{uuid:<salt>}}` ([`stable_uuid`]). Any other key is an error naming the
//! key and the declaring package.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

/// Which target of the package the bundle's executable is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BundleKind {
    /// The package's `cdylib`; `lib_name` is its library target's name.
    Cdylib { lib_name: String },
    /// The package's binary target `name`.
    Bin { name: String },
}

impl BundleKind {
    /// The file cargo produces for this target on macOS.
    pub fn file_name(&self) -> String {
        match self {
            Self::Cdylib { lib_name } => format!("lib{lib_name}.dylib"),
            Self::Bin { name } => name.clone(),
        }
    }
}

/// One `bundles` declaration, resolved against the dependency graph.
/// Template fields are still unrendered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BundleDecl {
    /// The package that declared it (named in errors).
    pub declarer: String,
    pub package: String,
    /// The package's source directory.
    pub manifest_dir: PathBuf,
    pub kind: BundleKind,
    pub features: Vec<String>,
    /// Bundle directory name (template).
    pub bundle: String,
    /// Directory inside `<App>.app` the bundle goes in.
    pub place: PathBuf,
    /// `Info.plist` template, absolute.
    pub info_plist: PathBuf,
    /// Entitlements template, absolute.
    pub entitlements: Option<PathBuf>,
    /// Nested bundle id (template).
    pub bundle_id: String,
    /// Build environment (values are templates).
    pub env: Vec<(String, String)>,
}

/// Everything the dependency graph declares for macOS.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SdkDeclarations {
    /// Boolean entitlements added to the app's signature.
    pub app_entitlements: Vec<String>,
    pub bundles: Vec<BundleDecl>,
}

/// A declared bundle whose package has been built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuiltBundle {
    pub decl: BundleDecl,
    /// The rendered bundle directory name.
    pub dir_name: String,
    /// The rendered nested bundle id.
    pub bundle_id: String,
    /// The built executable (universal when the app build is).
    pub executable: PathBuf,
}

/// What the app build produced for the SDKs' macOS declarations.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SdkBundles {
    /// Boolean entitlements the app's signature must carry.
    pub app_entitlements: Vec<String>,
    pub bundles: Vec<BuiltBundle>,
}

impl SdkBundles {
    pub fn is_empty(&self) -> bool {
        self.bundles.is_empty() && self.app_entitlements.is_empty()
    }
}

// ─────────────────────────────────────────────────────────────────────
// Discovery
// ─────────────────────────────────────────────────────────────────────

/// Read the macOS declarations from the wrapper's `cargo metadata`.
pub fn discover(wrapper_manifest: &Path) -> Result<SdkDeclarations> {
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
    resolve(&json)
}

/// The pure core of [`discover`]. A bundle naming a package that is not in
/// the graph, or a target the package does not have, is an error: shipping
/// without it would fail only when the OS goes looking for the bundle.
pub fn resolve(metadata: &Value) -> Result<SdkDeclarations> {
    let packages = metadata.get("packages").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut out = SdkDeclarations::default();
    for pkg in &packages {
        let Some(macos) = pkg.pointer("/metadata/idealyst/macos") else { continue };
        let declarer = pkg.get("name").and_then(Value::as_str).unwrap_or("?");
        for e in string_list(macos.get("app-entitlements"))
            .with_context(|| format!("{declarer}: macos `app-entitlements`"))?
        {
            if !out.app_entitlements.contains(&e) {
                out.app_entitlements.push(e);
            }
        }
        let decls = match macos.get("bundles") {
            None => continue,
            Some(Value::Array(d)) => d,
            Some(_) => bail!("{declarer}: `[package.metadata.idealyst.macos] bundles` must be an array of tables"),
        };
        for d in decls {
            out.bundles.push(resolve_bundle(declarer, d, &packages)?);
        }
    }
    Ok(out)
}

fn resolve_bundle(declarer: &str, d: &Value, packages: &[Value]) -> Result<BundleDecl> {
    let field = |key: &str| -> Result<String> {
        d.get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .with_context(|| format!("{declarer}: each macos bundles entry needs a string `{key}`"))
    };
    let package = field("package")?;
    let ctx = || format!("{declarer}: macos bundle from `{package}`");
    let Some(target_pkg) = packages.iter().find(|p| p.get("name").and_then(Value::as_str) == Some(package.as_str()))
    else {
        bail!("{declarer} declares a macos bundle from `{package}`, which is not in the dependency graph");
    };
    let manifest_dir = target_pkg
        .get("manifest_path")
        .and_then(Value::as_str)
        .and_then(|m| Path::new(m).parent().map(Path::to_path_buf))
        .with_context(|| format!("{package}: cargo metadata has no manifest_path"))?;
    let targets = target_pkg.get("targets").and_then(Value::as_array).cloned().unwrap_or_default();
    let kind = match field("kind")?.as_str() {
        "cdylib" => {
            let lib_name = targets.iter().find_map(|t| {
                let kinds = t.get("crate_types").or_else(|| t.get("kind"))?.as_array()?;
                if kinds.iter().any(|k| k.as_str() == Some("cdylib")) {
                    t.get("name").and_then(Value::as_str).map(str::to_string)
                } else {
                    None
                }
            });
            let Some(lib_name) = lib_name else {
                bail!("{}: the package's library is not a cdylib (add `crate-type = [\"rlib\", \"cdylib\"]`)", ctx());
            };
            BundleKind::Cdylib { lib_name }
        }
        "bin" => {
            let name = d
                .get("bin")
                .and_then(Value::as_str)
                .with_context(|| format!("{}: kind = \"bin\" needs `bin` (the binary target's name)", ctx()))?
                .to_string();
            let has_bin = targets.iter().any(|t| {
                t.get("name").and_then(Value::as_str) == Some(name.as_str())
                    && t.get("kind").and_then(Value::as_array).is_some_and(|k| k.iter().any(|k| k.as_str() == Some("bin")))
            });
            if !has_bin {
                bail!("{}: the package has no binary target `{name}`", ctx());
            }
            BundleKind::Bin { name }
        }
        other => bail!("{}: kind `{other}` is not \"cdylib\" or \"bin\"", ctx()),
    };
    let place = PathBuf::from(field("place")?);
    if !is_plain_relative(&place) {
        bail!("{}: `place` must be a relative path inside the .app, got {}", ctx(), place.display());
    }
    let relative_file = |key: &str, value: String| -> Result<PathBuf> {
        let p = PathBuf::from(&value);
        if !is_plain_relative(&p) {
            bail!("{}: `{key}` must be relative to the package's manifest dir, got {value}", ctx());
        }
        Ok(manifest_dir.join(p))
    };
    let info_plist = relative_file("info-plist", field("info-plist")?)?;
    let entitlements = match d.get("entitlements") {
        None => None,
        Some(v) => Some(relative_file(
            "entitlements",
            v.as_str().with_context(|| format!("{}: `entitlements` must be a string", ctx()))?.to_string(),
        )?),
    };
    let env = match d.get("env") {
        None => Vec::new(),
        Some(Value::Object(m)) => m
            .iter()
            .map(|(k, v)| {
                v.as_str()
                    .map(|v| (k.clone(), v.to_string()))
                    .with_context(|| format!("{}: env `{k}` must be a string", ctx()))
            })
            .collect::<Result<_>>()?,
        Some(_) => bail!("{}: `env` must be a table of strings", ctx()),
    };
    Ok(BundleDecl {
        declarer: declarer.to_string(),
        features: string_list(d.get("features")).with_context(|| format!("{}: `features`", ctx()))?,
        bundle: field("bundle")?,
        bundle_id: field("bundle-id")?,
        package,
        manifest_dir,
        kind,
        place,
        info_plist,
        entitlements,
        env,
    })
}

/// Relative, with no `..` — it cannot leave the directory it is joined to.
fn is_plain_relative(p: &Path) -> bool {
    !p.as_os_str().is_empty() && p.components().all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

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

// ─────────────────────────────────────────────────────────────────────
// Templates
// ─────────────────────────────────────────────────────────────────────

/// The app's identity, which templates draw on.
#[derive(Clone, Debug)]
pub struct AppIdentity<'a> {
    pub bundle_id: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    /// The Apple team. Only a template that uses `{{team_id}}` needs it.
    pub team_id: Option<&'a str>,
}

/// Replace every `{{key}}` in `template` with `lookup(key)`. `lookup`
/// returns `None` for an unknown key and `Some(Err)` for a known key with
/// no value here. `what` names the template in errors.
fn render(template: &str, what: &str, lookup: &dyn Fn(&str) -> Option<Result<String>>) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            bail!("{what}: unterminated `{{{{` template key");
        };
        let key = after[..end].trim();
        match lookup(key) {
            Some(value) => out.push_str(&value.with_context(|| format!("{what}: `{{{{{key}}}}}`"))?),
            None => bail!("{what}: unknown template key `{{{{{key}}}}}`"),
        }
        rest = &after[end + 2..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Render a declaration's `bundle`, `bundle-id` or `env` value: keys
/// `{{bundle_id}}` (the app's) and `{{app_name}}`.
pub fn render_name(template: &str, what: &str, app: &AppIdentity<'_>) -> Result<String> {
    render(template, what, &|key| match key {
        "bundle_id" => Some(Ok(app.bundle_id.to_string())),
        "app_name" => Some(Ok(app.name.to_string())),
        _ => None,
    })
}

/// Render a bundle's `Info.plist` / entitlements template. Both are XML:
/// values are XML-escaped, and `<!-- … -->` comments are copied verbatim
/// (a comment describing the `{{…}}` keys is not a use of one).
pub fn render_bundle_file(
    template: &str,
    what: &str,
    app: &AppIdentity<'_>,
    bundle_id: &str,
    executable: &str,
) -> Result<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    loop {
        let (text, comment) = match rest.find("<!--") {
            Some(at) => (&rest[..at], Some(&rest[at..])),
            None => (rest, None),
        };
        out.push_str(&render_xml_text(text, what, app, bundle_id, executable)?);
        let Some(comment) = comment else { break };
        let end = comment.find("-->").map_or(comment.len(), |e| e + 3);
        out.push_str(&comment[..end]);
        rest = &comment[end..];
    }
    Ok(out)
}

fn render_xml_text(
    template: &str,
    what: &str,
    app: &AppIdentity<'_>,
    bundle_id: &str,
    executable: &str,
) -> Result<String> {
    render(template, what, &|key| {
        Some(Ok(xml_escape(&match key {
            "bundle_id" => bundle_id.to_string(),
            "app_bundle_id" => app.bundle_id.to_string(),
            "app_name" => app.name.to_string(),
            "version" => app.version.to_string(),
            "executable" => executable.to_string(),
            "team_id" => {
                return Some(app.team_id.map(str::to_string).context(
                    "no Apple team could be resolved; set `IDEALYST_DEVELOPMENT_TEAM`, \
                     `[package.metadata.idealyst.app.links] apple_team_id`, or install a signing certificate",
                ))
            }
            _ => match key.strip_prefix("uuid:") {
                Some(salt) if !salt.is_empty() => stable_uuid(bundle_id, salt),
                _ => return None,
            },
        })))
    })
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// `{{uuid:<salt>}}`: a UUID that is the same on every build and release
/// for one (nested bundle id, salt), and differs between apps (whose
/// nested ids differ) — what a CFPlugIn factory id needs.
///
/// Derivation, fixed forever (a change re-registers every shipped plug-in
/// under a new id): SHA-256 of `idealyst-sdk-bundle-uuid` NUL `<bundle id>`
/// NUL `<salt>` (UTF-8); the first 16 bytes, with the version nibble set to
/// 8 and the variant bits to `10` (an RFC 9562 UUIDv8); uppercase
/// `8-4-4-4-12` hex.
pub fn stable_uuid(bundle_id: &str, salt: &str) -> String {
    let mut h = Sha256::new();
    h.update(b"idealyst-sdk-bundle-uuid\0");
    h.update(bundle_id.as_bytes());
    h.update(b"\0");
    h.update(salt.as_bytes());
    let digest = h.finalize();
    let mut b = [0u8; 16];
    b.copy_from_slice(&digest[..16]);
    b[6] = (b[6] & 0x0F) | 0x80;
    b[8] = (b[8] & 0x3F) | 0x80;
    let hex: String = b.iter().map(|x| format!("{x:02X}")).collect();
    format!("{}-{}-{}-{}-{}", &hex[0..8], &hex[8..12], &hex[12..16], &hex[16..20], &hex[20..32])
}

// ─────────────────────────────────────────────────────────────────────
// Building
// ─────────────────────────────────────────────────────────────────────

/// Where the app build happened, for building the bundles beside it.
pub struct BuildSite<'a> {
    pub wrapper_dir: &'a Path,
    pub target_dir: &'a Path,
    pub project_dir: &'a Path,
    pub patch_block: &'a str,
    pub release: bool,
    /// `Some(deployment target)` for a universal (lipo'd) build.
    pub universal: Option<&'a str>,
}

/// Build every declared bundle's package for the app's target.
pub fn build_all(decls: SdkDeclarations, app: &AppIdentity<'_>, site: &BuildSite<'_>) -> Result<SdkBundles> {
    let mut bundles = Vec::with_capacity(decls.bundles.len());
    for decl in decls.bundles {
        let what = |field: &str| format!("{}: macos bundle from `{}` `{field}`", decl.declarer, decl.package);
        let dir_name = render_name(&decl.bundle, &what("bundle"), app)?;
        if dir_name.is_empty() || dir_name.contains('/') {
            bail!("{}: renders to `{dir_name}`, not a directory name", what("bundle"));
        }
        let bundle_id = render_name(&decl.bundle_id, &what("bundle-id"), app)?;
        let env = decl
            .env
            .iter()
            .map(|(k, v)| Ok((k.clone(), render_name(v, &what(&format!("env.{k}")), app)?)))
            .collect::<Result<Vec<_>>>()?;
        let executable = build_one(&decl, &env, site)?;
        bundles.push(BuiltBundle { decl, dir_name, bundle_id, executable });
    }
    Ok(SdkBundles { app_entitlements: decls.app_entitlements, bundles })
}

fn build_one(decl: &BundleDecl, env: &[(String, String)], site: &BuildSite<'_>) -> Result<PathBuf> {
    let target = match &decl.kind {
        BundleKind::Cdylib { .. } => build_ios::sdk_products::ProductTarget::Lib,
        BundleKind::Bin { name } => build_ios::sdk_products::ProductTarget::Bin(name),
    };
    let file = decl.kind.file_name();
    let write = |p: &Path, s: &str| build_ios::write_if_changed(p, s).map(|_| ());
    let build = |triple: Option<&str>, env: &[(String, String)]| -> Result<PathBuf> {
        let b = build_ios::sdk_products::ProductBuild {
            label: "build-macos",
            wrapper_dir: site.wrapper_dir,
            target_dir: site.target_dir,
            project_dir: site.project_dir,
            patch_block: site.patch_block,
            package: &decl.package,
            manifest_dir: &decl.manifest_dir,
            target,
            features: &decl.features,
            env,
            triple,
            release: site.release,
        };
        let out = build_ios::sdk_products::build_product(&b, &write)?.join(&file);
        if !out.is_file() {
            bail!("{}: building `{}` produced no {}", decl.declarer, decl.package, out.display());
        }
        Ok(out)
    };
    let Some(deployment_target) = site.universal else {
        return build(None, env);
    };
    let mut env = env.to_vec();
    env.push(("MACOSX_DEPLOYMENT_TARGET".into(), deployment_target.into()));
    let mut slices = Vec::new();
    for triple in crate::UNIVERSAL_TARGETS {
        slices.push(build(Some(triple), &env)?);
    }
    let profile = if site.release { "release" } else { "debug" };
    let universal = site.target_dir.join(profile).join("sdk-products").join(&file);
    fs::create_dir_all(universal.parent().expect("has a parent"))?;
    crate::lipo_create(&slices, &universal)?;
    Ok(universal)
}

// ─────────────────────────────────────────────────────────────────────
// Assembly
// ─────────────────────────────────────────────────────────────────────

/// A nested bundle laid out inside an `.app`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssembledBundle {
    /// `<App>.app/<place>/<bundle>`.
    pub path: PathBuf,
    /// The rendered entitlements to sign it with, outside the bundle.
    pub entitlements: Option<PathBuf>,
}

/// Lay `built` out inside `app_dir`, replacing any previous copy, and
/// render its entitlements (if any) into `entitlements_dir`.
pub fn assemble(
    app_dir: &Path,
    built: &BuiltBundle,
    app: &AppIdentity<'_>,
    entitlements_dir: &Path,
) -> Result<AssembledBundle> {
    let decl = &built.decl;
    let what = |field: &str| format!("{}: macos bundle from `{}` `{field}`", decl.declarer, decl.package);
    let executable = built
        .executable
        .file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{}: executable {} has no name", what("kind"), built.executable.display()))?
        .to_string();
    let read = |p: &Path, field: &str| {
        fs::read_to_string(p).with_context(|| format!("{}: read {}", what(field), p.display()))
    };
    let plist = render_bundle_file(&read(&decl.info_plist, "info-plist")?, &what("info-plist"), app, &built.bundle_id, &executable)?;
    let entitlements = match &decl.entitlements {
        None => None,
        Some(p) => Some(render_bundle_file(&read(p, "entitlements")?, &what("entitlements"), app, &built.bundle_id, &executable)?),
    };

    let path = app_dir.join(&decl.place).join(&built.dir_name);
    // Replaced whole: a file left from an earlier layout would sit inside
    // the bundle's seal.
    if path.exists() {
        fs::remove_dir_all(&path).with_context(|| format!("remove stale {}", path.display()))?;
    }
    let macos = path.join("Contents").join("MacOS");
    fs::create_dir_all(&macos).with_context(|| format!("create {}", macos.display()))?;
    let dest = macos.join(&executable);
    fs::copy(&built.executable, &dest)
        .with_context(|| format!("copy {} → {}", built.executable.display(), dest.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o755))?;
    }
    fs::write(path.join("Contents").join("Info.plist"), plist)?;

    let entitlements = match entitlements {
        None => None,
        Some(text) => {
            fs::create_dir_all(entitlements_dir)?;
            let file = entitlements_dir.join(format!("{}.entitlements", built.dir_name));
            fs::write(&file, text).with_context(|| format!("write {}", file.display()))?;
            Some(file)
        }
    };
    Ok(AssembledBundle { path, entitlements })
}

/// The union of `existing` and `extra`, sorted, without duplicates.
pub fn union_entitlements(existing: &[String], extra: &[String]) -> Vec<String> {
    let mut all: BTreeMap<&str, ()> = BTreeMap::new();
    for e in existing.iter().chain(extra) {
        all.insert(e, ());
    }
    all.into_keys().map(str::to_string).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The shape an SDK with an audio plug-in and a camera extension
    /// declares (the cargo-metadata JSON of its two tables).
    fn metadata() -> Value {
        json!({ "packages": [
            { "name": "devices-sdk", "manifest_path": "/w/devices-sdk/Cargo.toml",
              "metadata": { "idealyst": { "macos": {
                  "app-entitlements": ["com.apple.developer.system-extension.install"],
                  "bundles": [
                      { "package": "audio-plugin", "kind": "cdylib", "features": ["plugin"],
                        "bundle": "{{app_name}} Audio.driver", "place": "Contents/Resources",
                        "info-plist": "bundle/Info.plist", "bundle-id": "{{bundle_id}}.audio-plugin",
                        "env": { "VENDOR": "{{app_name}}" } },
                      { "package": "camera-ext", "kind": "bin", "bin": "camera-extension",
                        "features": ["extension"],
                        "bundle": "{{bundle_id}}.camera-extension.systemextension",
                        "place": "Contents/Library/SystemExtensions",
                        "info-plist": "bundle/Info.plist", "entitlements": "bundle/extension.entitlements",
                        "bundle-id": "{{bundle_id}}.camera-extension" },
                  ] } } } },
            { "name": "audio-plugin", "manifest_path": "/reg/audio-plugin-1.0.0/Cargo.toml",
              "targets": [ { "name": "audio_plugin", "kind": ["rlib", "cdylib"], "crate_types": ["rlib", "cdylib"] } ] },
            { "name": "camera-ext", "manifest_path": "/reg/camera-ext-1.0.0/Cargo.toml",
              "targets": [ { "name": "camera_ext", "kind": ["lib"], "crate_types": ["lib"] },
                           { "name": "camera-extension", "kind": ["bin"], "crate_types": ["bin"] } ] },
        ]})
    }

    fn app() -> AppIdentity<'static> {
        AppIdentity { bundle_id: "com.acme.demo", name: "Demo", version: "1.2.3", team_id: Some("TEAM123456") }
    }

    #[test]
    fn declarations_resolve_to_a_build_plan() {
        let d = resolve(&metadata()).unwrap();
        assert_eq!(d.app_entitlements, ["com.apple.developer.system-extension.install"]);
        assert_eq!(d.bundles.len(), 2);
        let audio = &d.bundles[0];
        assert_eq!(audio.declarer, "devices-sdk");
        assert_eq!(audio.kind, BundleKind::Cdylib { lib_name: "audio_plugin".into() });
        assert_eq!(audio.kind.file_name(), "libaudio_plugin.dylib");
        assert_eq!(audio.features, ["plugin"]);
        assert_eq!(audio.info_plist, PathBuf::from("/reg/audio-plugin-1.0.0/bundle/Info.plist"));
        assert_eq!(audio.entitlements, None);
        assert_eq!(audio.place, PathBuf::from("Contents/Resources"));
        assert_eq!(audio.env, [("VENDOR".to_string(), "{{app_name}}".to_string())]);
        let cam = &d.bundles[1];
        assert_eq!(cam.kind, BundleKind::Bin { name: "camera-extension".into() });
        assert_eq!(cam.entitlements, Some(PathBuf::from("/reg/camera-ext-1.0.0/bundle/extension.entitlements")));
    }

    #[test]
    fn a_bundle_from_a_package_outside_the_graph_is_an_error() {
        let mut m = metadata();
        m["packages"].as_array_mut().unwrap().remove(2);
        let err = resolve(&m).unwrap_err().to_string();
        assert!(err.contains("camera-ext") && err.contains("not in the dependency graph"), "{err}");
    }

    #[test]
    fn a_bin_bundle_needs_an_existing_bin_target() {
        let mut m = metadata();
        let cam = &mut m["packages"][0]["metadata"]["idealyst"]["macos"]["bundles"][1];
        cam.as_object_mut().unwrap().remove("bin");
        assert!(resolve(&m).unwrap_err().to_string().contains("needs `bin`"));
        let mut m = metadata();
        m["packages"][0]["metadata"]["idealyst"]["macos"]["bundles"][1]["bin"] = json!("nope");
        assert!(resolve(&m).unwrap_err().to_string().contains("no binary target `nope`"));
    }

    #[test]
    fn a_cdylib_bundle_needs_a_cdylib_library() {
        let mut m = metadata();
        m["packages"][1]["targets"][0]["crate_types"] = json!(["rlib"]);
        m["packages"][1]["targets"][0]["kind"] = json!(["lib"]);
        assert!(resolve(&m).unwrap_err().to_string().contains("not a cdylib"));
    }

    #[test]
    fn bundle_paths_cannot_escape() {
        for (key, value) in [("place", "../outside"), ("place", "/abs"), ("info-plist", "../../etc/x")] {
            let mut m = metadata();
            m["packages"][0]["metadata"]["idealyst"]["macos"]["bundles"][0][key] = json!(value);
            assert!(resolve(&m).is_err(), "{key} = {value} must be rejected");
        }
    }

    #[test]
    fn name_templates_use_the_app_identity() {
        let a = app();
        assert_eq!(render_name("{{app_name}} Audio.driver", "t", &a).unwrap(), "Demo Audio.driver");
        assert_eq!(render_name("{{bundle_id}}.camera-extension", "t", &a).unwrap(), "com.acme.demo.camera-extension");
        // Bundle-file keys are not name keys.
        assert!(render_name("{{version}}", "t", &a).is_err());
    }

    #[test]
    fn bundle_file_templates_fill_every_key() {
        let out = render_bundle_file(
            "{{bundle_id}}|{{app_bundle_id}}|{{app_name}}|{{version}}|{{executable}}|{{team_id}}|{{ uuid:factory }}",
            "t",
            &app(),
            "com.acme.demo.audio-plugin",
            "libaudio_plugin.dylib",
        )
        .unwrap();
        assert_eq!(
            out,
            format!(
                "com.acme.demo.audio-plugin|com.acme.demo|Demo|1.2.3|libaudio_plugin.dylib|TEAM123456|{}",
                stable_uuid("com.acme.demo.audio-plugin", "factory")
            )
        );
    }

    #[test]
    fn an_unknown_template_key_is_an_error_naming_key_and_declarer() {
        let err = render_bundle_file("<string>{{bundle_version}}</string>", "devices-sdk: macos bundle from `audio-plugin` `info-plist`", &app(), "x", "y")
            .unwrap_err()
            .to_string();
        assert!(err.contains("{{bundle_version}}") && err.contains("devices-sdk"), "{err}");
        assert!(render_name("{{nope}}", "devices-sdk: bundle", &app()).unwrap_err().to_string().contains("{{nope}}"));
        assert!(render_name("{{app_name", "t", &app()).unwrap_err().to_string().contains("unterminated"));
    }

    /// The camera SDK's Info.plist opens with a comment describing "the
    /// {{…}} keys", which failed the build as an unknown key.
    #[test]
    fn regression_template_keys_in_xml_comments_are_not_rendered() {
        let t = "<!-- the CLI fills the {{…}} keys, e.g. {{bundle_id}} -->\n<string>{{bundle_id}}</string><!-- {{x}} -->";
        assert_eq!(
            render_bundle_file(t, "t", &app(), "a.b", "e").unwrap(),
            "<!-- the CLI fills the {{…}} keys, e.g. {{bundle_id}} -->\n<string>a.b</string><!-- {{x}} -->"
        );
    }

    #[test]
    fn bundle_file_values_are_xml_escaped() {
        let a = AppIdentity { name: "Tom & Jerry <TV>", ..app() };
        assert_eq!(render_bundle_file("<string>{{app_name}}</string>", "t", &a, "a.b", "e").unwrap(), "<string>Tom &amp; Jerry &lt;TV&gt;</string>");
        // A name template renders a path / env value, which is not XML.
        assert_eq!(render_name("{{app_name}} Audio.driver", "t", &a).unwrap(), "Tom & Jerry <TV> Audio.driver");
    }

    #[test]
    fn team_id_is_needed_only_when_a_template_uses_it() {
        let no_team = AppIdentity { team_id: None, ..app() };
        assert!(render_bundle_file("{{bundle_id}}", "t", &no_team, "a.b", "e").is_ok());
        let err = render_bundle_file("{{team_id}}.{{bundle_id}}", "t", &no_team, "a.b", "e").unwrap_err();
        assert!(format!("{err:#}").contains("no Apple team"), "{err:#}");
    }

    /// The CFPlugIn factory id an installed plug-in registered under must
    /// survive every rebuild and release of the CLI: pinned to literals.
    #[test]
    fn stable_uuid_never_changes() {
        assert_eq!(stable_uuid("com.acme.demo.audio-plugin", "factory"), "4ECE68F6-27BE-882B-AB17-9B9E6BA0611A");
        assert_eq!(stable_uuid("com.acme.demo.audio-plugin", "factory"), stable_uuid("com.acme.demo.audio-plugin", "factory"));
        assert_ne!(stable_uuid("com.acme.demo.audio-plugin", "factory"), stable_uuid("com.acme.other.audio-plugin", "factory"));
        assert_ne!(stable_uuid("com.acme.demo.audio-plugin", "factory"), stable_uuid("com.acme.demo.audio-plugin", "other"));
        let u = stable_uuid("x", "y");
        assert_eq!(u.len(), 36);
        assert_eq!(&u[14..15], "8", "version 8");
        assert!("89AB".contains(&u[19..20]), "RFC variant");
        assert_eq!(u, u.to_uppercase());
    }

    #[test]
    fn assemble_lays_out_the_bundle_and_renders_its_files() {
        let tmp = tempfile::tempdir().unwrap();
        let pkg = tmp.path().join("camera-ext");
        fs::create_dir_all(pkg.join("bundle")).unwrap();
        fs::write(pkg.join("bundle/Info.plist"), "<id>{{bundle_id}}</id><exe>{{executable}}</exe><svc>{{team_id}}.{{bundle_id}}</svc>").unwrap();
        fs::write(pkg.join("bundle/extension.entitlements"), "<group>{{team_id}}.{{bundle_id}}</group>").unwrap();
        let built_exe = tmp.path().join("target/debug/camera-extension");
        fs::create_dir_all(built_exe.parent().unwrap()).unwrap();
        fs::write(&built_exe, b"mach-o").unwrap();
        let app_dir = tmp.path().join("target/debug/Demo.app");
        let built = BuiltBundle {
            decl: BundleDecl {
                declarer: "devices-sdk".into(),
                package: "camera-ext".into(),
                manifest_dir: pkg.clone(),
                kind: BundleKind::Bin { name: "camera-extension".into() },
                features: vec![],
                bundle: String::new(),
                place: PathBuf::from("Contents/Library/SystemExtensions"),
                info_plist: pkg.join("bundle/Info.plist"),
                entitlements: Some(pkg.join("bundle/extension.entitlements")),
                bundle_id: String::new(),
                env: vec![],
            },
            dir_name: "com.acme.demo.camera-extension.systemextension".into(),
            bundle_id: "com.acme.demo.camera-extension".into(),
            executable: built_exe,
        };
        let ent_dir = tmp.path().join("target/debug/sdk-entitlements");
        // A file from an earlier layout must not survive into the seal.
        let stale = app_dir.join("Contents/Library/SystemExtensions/com.acme.demo.camera-extension.systemextension/stale");
        fs::create_dir_all(stale.parent().unwrap()).unwrap();
        fs::write(&stale, b"old").unwrap();

        let out = assemble(&app_dir, &built, &app(), &ent_dir).unwrap();
        assert_eq!(out.path, app_dir.join("Contents/Library/SystemExtensions/com.acme.demo.camera-extension.systemextension"));
        assert!(!stale.exists());
        assert_eq!(fs::read(out.path.join("Contents/MacOS/camera-extension")).unwrap(), b"mach-o");
        assert_eq!(
            fs::read_to_string(out.path.join("Contents/Info.plist")).unwrap(),
            "<id>com.acme.demo.camera-extension</id><exe>camera-extension</exe>\
             <svc>TEAM123456.com.acme.demo.camera-extension</svc>"
        );
        let ent = out.entitlements.expect("rendered entitlements");
        assert!(!ent.starts_with(&app_dir), "entitlements stay outside the signed bundle");
        assert_eq!(fs::read_to_string(ent).unwrap(), "<group>TEAM123456.com.acme.demo.camera-extension</group>");
    }

    #[test]
    fn app_entitlements_union_is_sorted_and_deduplicated() {
        assert_eq!(
            union_entitlements(&["b".into(), "a".into()], &["a".into(), "c".into()]),
            ["a", "b", "c"]
        );
    }
}
