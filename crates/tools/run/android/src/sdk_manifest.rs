//! Manifest fragments and resources contributed by SDK crates.
//!
//! Capabilities cover the common case (a `<uses-permission>`), but some SDKs
//! need components the OS instantiates by name — a `<receiver>` a device
//! owner is bound through, an `<activity>` the setup wizard launches — plus
//! the resource files those components point at. An SDK declares them in its
//! own `Cargo.toml`:
//!
//! ```toml
//! [package.metadata.idealyst.android]
//! # XML fragments spliced inside <application>.
//! manifest_application = ["manifest/application.xml"]
//! # XML fragments spliced inside <manifest>, before <application>.
//! manifest_root = ["manifest/root.xml"]
//! # Resource files, each `res/<type>/<file>`, compiled into the APK.
//! res = ["res/xml/device_admin.xml"]
//! ```
//!
//! Paths resolve against the declaring package's directory, like
//! `runtime_kotlin`. A fragment is a sequence of elements (no root element,
//! no XML declaration) using the `android:` prefix freely — it is validated
//! as XML at build time, and each top-level element must be one that is
//! legal at its splice point, so a `<receiver>` declared under
//! `manifest_root` fails with the SDK's name rather than as an opaque
//! `aapt2 link` error. `${applicationId}` in a fragment is replaced with the
//! app's package name.
//!
//! The fragments go into every app whose dependency graph contains the SDK:
//! a component an SDK declares exists in each such app, so SDKs should only
//! declare components that are inert until something enables them (a
//! device-admin receiver does nothing until the app is made an admin).
//! App-specific choices — whether the main activity is a launcher, its lock
//! task mode — are the app's, in `[package.metadata.idealyst.app.android]`.
//! So is the wording of an SDK's user-visible strings: an SDK ships a
//! default in a `res/values*/` file and the app replaces it by name in
//! `[package.metadata.idealyst.app.android.strings]`
//! ([`SdkManifestContributions::stage_string_overrides`]).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde_json::Value;

/// Elements an SDK may splice directly inside `<manifest>`.
const ROOT_ELEMENTS: &[&str] = &[
    "uses-permission",
    "uses-permission-sdk-23",
    "permission",
    "permission-group",
    "permission-tree",
    "uses-feature",
    "queries",
    "supports-screens",
];

/// Elements an SDK may splice directly inside `<application>`.
const APPLICATION_ELEMENTS: &[&str] = &[
    "activity",
    "activity-alias",
    "receiver",
    "service",
    "provider",
    "meta-data",
    "uses-library",
    "uses-native-library",
    "profileable",
    "property",
];

const ANDROID_NS: &str = "http://schemas.android.com/apk/res/android";
const TOOLS_NS: &str = "http://schemas.android.com/tools";

/// One XML fragment, read and validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fragment {
    /// The crate that declared it (diagnostics, and a comment in the output).
    pub package: String,
    pub xml: String,
}

/// One resource file to compile into the APK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResFile {
    pub package: String,
    pub source: PathBuf,
    /// Path under a `res/` root: `<type>/<file>`.
    pub staged: PathBuf,
}

/// Everything the dependency graph's SDKs contribute to the manifest and
/// resource table.
#[derive(Debug, Default)]
pub struct SdkManifestContributions {
    pub manifest_root: Vec<Fragment>,
    pub manifest_application: Vec<Fragment>,
    pub res: Vec<ResFile>,
}

impl SdkManifestContributions {
    /// Collect every package's contributions from `cargo metadata` JSON.
    pub fn from_metadata(json: &Value) -> Result<Self> {
        let mut out = Self::default();
        let Some(packages) = json.get("packages").and_then(|v| v.as_array()) else {
            return Ok(out);
        };
        let mut staged_owner: BTreeMap<PathBuf, String> = BTreeMap::new();
        for pkg in packages {
            let Some(android) = pkg
                .pointer("/metadata/idealyst/android")
                .and_then(|v| v.as_object())
            else {
                continue;
            };
            let name = pkg.get("name").and_then(|v| v.as_str()).unwrap_or("<unknown>");
            let dir = pkg
                .get("manifest_path")
                .and_then(|v| v.as_str())
                .and_then(|p| Path::new(p).parent())
                .ok_or_else(|| anyhow::anyhow!("package {name} has no manifest_path"))?
                .to_path_buf();

            for rel in string_list(android, "manifest_root", name)? {
                out.manifest_root
                    .push(read_fragment(name, &dir, &rel, "manifest_root", ROOT_ELEMENTS)?);
            }
            for rel in string_list(android, "manifest_application", name)? {
                out.manifest_application.push(read_fragment(
                    name,
                    &dir,
                    &rel,
                    "manifest_application",
                    APPLICATION_ELEMENTS,
                )?);
            }
            for rel in string_list(android, "res", name)? {
                let staged = res_staged_path(name, &rel)?;
                let source = dir.join(&rel);
                if !source.is_file() {
                    bail!(
                        "{name} declares resource {} (via metadata.idealyst.android.res) but the \
                         file does not exist",
                        source.display()
                    );
                }
                if let Some(owner) = staged_owner.insert(staged.clone(), name.to_string()) {
                    bail!(
                        "res/{} is declared by both {owner} and {name}; resource names share one \
                         namespace in the APK",
                        staged.display()
                    );
                }
                out.res.push(ResFile {
                    package: name.to_string(),
                    source,
                    staged,
                });
            }
        }
        Ok(out)
    }

    /// The `<manifest>`-level splice, `${applicationId}` substituted.
    pub fn render_root(&self, application_id: &str) -> String {
        render_fragments(&self.manifest_root, application_id)
    }

    /// The `<application>`-level splice, `${applicationId}` substituted.
    pub fn render_application(&self, application_id: &str) -> String {
        render_fragments(&self.manifest_application, application_id)
    }

    /// Copy the resource files into `res_root` (`<res_root>/<type>/<file>`),
    /// replacing whatever was there. Returns `false` when there is nothing to
    /// stage, so the caller can skip the `aapt2 compile`.
    pub fn stage_res(&self, res_root: &Path) -> Result<bool> {
        if res_root.exists() {
            fs::remove_dir_all(res_root)
                .with_context(|| format!("clear {}", res_root.display()))?;
        }
        if self.res.is_empty() {
            return Ok(false);
        }
        for f in &self.res {
            let dest = res_root.join(&f.staged);
            fs::create_dir_all(dest.parent().expect("staged path has a type dir"))?;
            fs::copy(&f.source, &dest).with_context(|| {
                format!("stage {} (from {}) → {}", f.source.display(), f.package, dest.display())
            })?;
        }
        Ok(true)
    }
}

impl SdkManifestContributions {
    /// The `<string>` names the SDKs' `res/values*/` files declare.
    pub fn declared_strings(&self) -> Result<BTreeSet<String>> {
        let mut names = BTreeSet::new();
        for f in &self.res {
            let is_values = f
                .staged
                .parent()
                .and_then(|d| d.to_str())
                .is_some_and(|d| d == "values" || d.starts_with("values-"));
            if !is_values {
                continue;
            }
            let xml = fs::read_to_string(&f.source)
                .with_context(|| format!("read {} (from {})", f.source.display(), f.package))?;
            let doc = roxmltree::Document::parse(&xml)
                .with_context(|| format!("{}: {} is not well-formed XML", f.package, f.source.display()))?;
            names.extend(
                doc.root_element()
                    .children()
                    .filter(|n| n.has_tag_name("string"))
                    .filter_map(|n| n.attribute("name").map(str::to_string)),
            );
        }
        Ok(names)
    }

    /// Write the app's string overrides to `<res_root>/values/strings.xml`,
    /// replacing whatever was there. They are linked after the SDK
    /// resources, as an overlay, so each replaces the SDK's default. A name
    /// no SDK declares is an error: it would add an unused resource and
    /// leave the default the app meant to replace. Returns `false` when
    /// there is nothing to stage.
    pub fn stage_string_overrides(
        &self,
        strings: &BTreeMap<String, String>,
        res_root: &Path,
    ) -> Result<bool> {
        if res_root.exists() {
            fs::remove_dir_all(res_root)
                .with_context(|| format!("clear {}", res_root.display()))?;
        }
        if strings.is_empty() {
            return Ok(false);
        }
        let declared = self.declared_strings()?;
        let unknown: Vec<&str> = strings
            .keys()
            .map(String::as_str)
            .filter(|k| !declared.contains(*k))
            .collect();
        if !unknown.is_empty() {
            bail!(
                "`[package.metadata.idealyst.app.android.strings]` names {} that no SDK in the \
                 dependency graph declares; declared: {}",
                unknown.join(", "),
                if declared.is_empty() {
                    "none".to_string()
                } else {
                    declared.iter().cloned().collect::<Vec<_>>().join(", ")
                }
            );
        }
        let mut xml = String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<resources>\n");
        for (name, value) in strings {
            xml.push_str(&format!(
                "    <string name=\"{name}\">{}</string>\n",
                android_string_escape(value)
            ));
        }
        xml.push_str("</resources>\n");
        let dest = res_root.join("values/strings.xml");
        fs::create_dir_all(dest.parent().expect("has a values dir"))?;
        fs::write(&dest, xml).with_context(|| format!("write {}", dest.display()))?;
        Ok(true)
    }
}

/// A string as an Android `<string>` element's content: XML-escaped, then
/// Android's own escapes — `\'` and `\"` (aapt2 rejects a bare apostrophe
/// and drops bare quotes), `\\`, `\n`, and a leading `@` or `?` that would
/// otherwise read as a resource reference.
pub(crate) fn android_string_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for (i, c) in value.chars().enumerate() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '@' | '?' if i == 0 => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out
}

/// `flat` when `staged` (it was just compiled from fresh input), else
/// `None` with any copy from an earlier build deleted, so an input removed
/// since then (an SDK dropped from the graph, an override deleted) is not
/// linked again from the stale file.
pub(crate) fn fresh_flat(flat: &Path, staged: bool) -> Result<Option<PathBuf>> {
    if staged {
        return Ok(Some(flat.to_path_buf()));
    }
    match fs::remove_file(flat) {
        Ok(()) => Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("remove stale {}", flat.display())),
    }
}

fn render_fragments(fragments: &[Fragment], application_id: &str) -> String {
    fragments
        .iter()
        .map(|f| {
            format!(
                "<!-- from {} -->\n{}",
                f.package,
                f.xml.trim().replace("${applicationId}", application_id)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn string_list(
    android: &serde_json::Map<String, Value>,
    key: &str,
    pkg: &str,
) -> Result<Vec<String>> {
    let Some(v) = android.get(key) else {
        return Ok(Vec::new());
    };
    let arr = v
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("{pkg}: metadata.idealyst.android.{key} must be an array"))?;
    arr.iter()
        .map(|e| {
            e.as_str().map(str::to_string).ok_or_else(|| {
                anyhow::anyhow!("{pkg}: metadata.idealyst.android.{key} entries must be strings")
            })
        })
        .collect()
}

fn check_relative(pkg: &str, key: &str, rel: &str) -> Result<()> {
    let p = Path::new(rel);
    if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        bail!("{pkg}: metadata.idealyst.android.{key} entry {rel:?} must be relative to the package and not use `..`");
    }
    Ok(())
}

fn read_fragment(
    pkg: &str,
    dir: &Path,
    rel: &str,
    key: &str,
    allowed: &[&str],
) -> Result<Fragment> {
    check_relative(pkg, key, rel)?;
    let path = dir.join(rel);
    let xml = fs::read_to_string(&path).with_context(|| {
        format!("{pkg} declares {} (via metadata.idealyst.android.{key}) but it cannot be read", path.display())
    })?;
    validate_fragment(&xml, allowed)
        .with_context(|| format!("{pkg}: {} (metadata.idealyst.android.{key})", path.display()))?;
    Ok(Fragment {
        package: pkg.to_string(),
        xml,
    })
}

/// Parse `xml` as a sequence of elements and check each top-level element
/// is legal at the splice point.
fn validate_fragment(xml: &str, allowed: &[&str]) -> Result<()> {
    if xml.trim_start().starts_with("<?xml") {
        bail!("a manifest fragment must not carry an XML declaration");
    }
    let wrapped = format!(
        "<fragment xmlns:android=\"{ANDROID_NS}\" xmlns:tools=\"{TOOLS_NS}\">{xml}</fragment>"
    );
    let doc = roxmltree::Document::parse(&wrapped).context("not well-formed XML")?;
    let mut any = false;
    for child in doc.root_element().children() {
        if child.is_text() && !child.text().unwrap_or_default().trim().is_empty() {
            bail!("text outside an element: {:?}", child.text().unwrap_or_default().trim());
        }
        if !child.is_element() {
            continue;
        }
        any = true;
        let tag = child.tag_name().name();
        if !allowed.contains(&tag) {
            bail!(
                "<{tag}> is not allowed here; this splice point accepts {}",
                allowed.iter().map(|t| format!("<{t}>")).collect::<Vec<_>>().join(", ")
            );
        }
    }
    if !any {
        bail!("the fragment declares no elements");
    }
    Ok(())
}

/// `res/<type>/<file>` → `<type>/<file>`.
fn res_staged_path(pkg: &str, rel: &str) -> Result<PathBuf> {
    check_relative(pkg, "res", rel)?;
    let parts: Vec<&str> = rel.split('/').collect();
    let valid_type = |t: &str| {
        !t.is_empty()
            && t.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '+')
    };
    let valid_file = |f: &str| {
        let stem = f.split('.').next().unwrap_or_default();
        !stem.is_empty()
            && stem.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
    };
    if parts.len() != 3 || parts[0] != "res" || !valid_type(parts[1]) || !valid_file(parts[2]) {
        bail!(
            "{pkg}: metadata.idealyst.android.res entry {rel:?} must be `res/<type>/<file>` \
             with a lowercase file name (e.g. `res/xml/device_admin.xml`)"
        );
    }
    Ok(PathBuf::from(parts[1]).join(parts[2]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sdk(dir: &Path, android: Value) -> Value {
        json!({ "packages": [{
            "name": "fake-sdk",
            "manifest_path": dir.join("Cargo.toml").display().to_string(),
            "metadata": { "idealyst": { "android": android } },
        }]})
    }

    fn write(dir: &Path, rel: &str, body: &str) {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    const RECEIVER: &str = r#"<receiver android:name="io.fake.Admin"
    android:permission="android.permission.BIND_DEVICE_ADMIN" android:exported="true">
  <meta-data android:name="android.app.device_admin" android:resource="@xml/device_admin" />
</receiver>"#;

    #[test]
    fn fragments_and_res_are_collected_from_sdk_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "manifest/app.xml", RECEIVER);
        write(
            tmp.path(),
            "manifest/root.xml",
            r#"<uses-permission android:name="${applicationId}.permission.X" />"#,
        );
        write(tmp.path(), "res/xml/device_admin.xml", "<device-admin/>");
        let c = SdkManifestContributions::from_metadata(&sdk(
            tmp.path(),
            json!({
                "manifest_application": ["manifest/app.xml"],
                "manifest_root": ["manifest/root.xml"],
                "res": ["res/xml/device_admin.xml"],
            }),
        ))
        .unwrap();
        let app = c.render_application("com.example.agent");
        assert!(app.starts_with("<!-- from fake-sdk -->"), "{app}");
        assert!(app.contains("io.fake.Admin"));
        assert_eq!(
            c.render_root("com.example.agent"),
            "<!-- from fake-sdk -->\n<uses-permission android:name=\"com.example.agent.permission.X\" />"
        );
        assert_eq!(c.res[0].staged, PathBuf::from("xml/device_admin.xml"));

        let res_root = tmp.path().join("staged-res");
        assert!(c.stage_res(&res_root).unwrap());
        assert_eq!(
            fs::read_to_string(res_root.join("xml/device_admin.xml")).unwrap(),
            "<device-admin/>"
        );
    }

    const STRINGS: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<resources>
    <string name="svc_label">Default label</string>
    <string name="svc_description">Default description</string>
</resources>"#;

    fn with_strings(tmp: &Path) -> SdkManifestContributions {
        write(tmp, "res/values/svc_strings.xml", STRINGS);
        write(tmp, "res/xml/svc.xml", "<svc/>");
        SdkManifestContributions::from_metadata(&sdk(
            tmp,
            json!({ "res": ["res/xml/svc.xml", "res/values/svc_strings.xml"] }),
        ))
        .unwrap()
    }

    #[test]
    fn declared_strings_come_from_values_files_only() {
        let tmp = tempfile::tempdir().unwrap();
        let c = with_strings(tmp.path());
        let names: Vec<String> = c.declared_strings().unwrap().into_iter().collect();
        assert_eq!(names, ["svc_description", "svc_label"]);
    }

    #[test]
    fn string_overrides_are_written_escaped_for_aapt2() {
        let tmp = tempfile::tempdir().unwrap();
        let c = with_strings(tmp.path());
        let out = tmp.path().join("app-strings");
        let strings = BTreeMap::from([
            ("svc_label".to_string(), "Bob's \"remote\" & <support>".to_string()),
            ("svc_description".to_string(), "@not a ref\nline two".to_string()),
        ]);
        assert!(c.stage_string_overrides(&strings, &out).unwrap());
        let xml = fs::read_to_string(out.join("values/strings.xml")).unwrap();
        assert!(xml.contains(r#"<string name="svc_label">Bob\'s \"remote\" &amp; &lt;support&gt;</string>"#), "{xml}");
        assert!(xml.contains(r#"<string name="svc_description">\@not a ref\nline two</string>"#), "{xml}");
        roxmltree::Document::parse(&xml).expect("well-formed");
    }

    #[test]
    fn an_override_no_sdk_declares_fails_and_lists_the_declared_names() {
        let tmp = tempfile::tempdir().unwrap();
        let c = with_strings(tmp.path());
        let strings = BTreeMap::from([("svc_lable".to_string(), "typo".to_string())]);
        let err = c
            .stage_string_overrides(&strings, &tmp.path().join("app-strings"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("svc_lable") && err.contains("svc_label"), "{err}");
    }

    #[test]
    fn no_overrides_stage_nothing_and_clear_an_earlier_build() {
        let tmp = tempfile::tempdir().unwrap();
        let c = with_strings(tmp.path());
        let out = tmp.path().join("app-strings");
        write(&out, "values/strings.xml", "<resources/>");
        assert!(!c.stage_string_overrides(&BTreeMap::new(), &out).unwrap());
        assert!(!out.exists());
    }

    #[test]
    fn regression_a_removed_resource_input_is_not_linked_from_a_stale_flat() {
        // A build with SDK resources leaves `sdk-res.flata` behind; a later
        // build with none must not link it again.
        let tmp = tempfile::tempdir().unwrap();
        let flat = tmp.path().join("sdk-res.flata");
        fs::write(&flat, b"stale").unwrap();
        assert_eq!(fresh_flat(&flat, false).unwrap(), None);
        assert!(!flat.exists());
        assert_eq!(fresh_flat(&flat, false).unwrap(), None, "already gone is fine");
        fs::write(&flat, b"fresh").unwrap();
        assert_eq!(fresh_flat(&flat, true).unwrap(), Some(flat.clone()));
        assert!(flat.exists());
    }

    #[test]
    fn a_component_at_the_wrong_splice_point_names_the_sdk() {
        let tmp = tempfile::tempdir().unwrap();
        write(tmp.path(), "manifest/root.xml", RECEIVER);
        let err = SdkManifestContributions::from_metadata(&sdk(
            tmp.path(),
            json!({ "manifest_root": ["manifest/root.xml"] }),
        ))
        .unwrap_err();
        let msg = format!("{err:#}");
        assert!(msg.contains("fake-sdk"), "{msg}");
        assert!(msg.contains("<receiver> is not allowed here"), "{msg}");
    }

    #[test]
    fn malformed_fragments_are_rejected() {
        assert!(validate_fragment("<receiver android:name=\"x\">", APPLICATION_ELEMENTS).is_err());
        assert!(validate_fragment("stray <receiver/>", APPLICATION_ELEMENTS).is_err());
        assert!(validate_fragment("<!-- only a comment -->", APPLICATION_ELEMENTS).is_err());
        assert!(validate_fragment("<?xml version=\"1.0\"?><receiver/>", APPLICATION_ELEMENTS).is_err());
        assert!(validate_fragment("<receiver/><!-- c --><activity/>", APPLICATION_ELEMENTS).is_ok());
    }

    #[test]
    fn res_paths_must_be_type_and_file() {
        assert_eq!(res_staged_path("p", "res/xml/a_b.xml").unwrap(), PathBuf::from("xml/a_b.xml"));
        assert_eq!(
            res_staged_path("p", "res/values-v28/strings.xml").unwrap(),
            PathBuf::from("values-v28/strings.xml")
        );
        assert!(res_staged_path("p", "xml/a.xml").is_err());
        assert!(res_staged_path("p", "res/xml/Admin.xml").is_err());
        assert!(res_staged_path("p", "res/../x/a.xml").is_err());
        assert!(res_staged_path("p", "res/a.xml").is_err());
    }

    #[test]
    fn two_sdks_declaring_one_resource_is_an_error() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(a.path(), "res/xml/policy.xml", "<a/>");
        write(b.path(), "res/xml/policy.xml", "<b/>");
        let meta = json!({ "packages": [
            { "name": "sdk-a", "manifest_path": a.path().join("Cargo.toml").display().to_string(),
              "metadata": { "idealyst": { "android": { "res": ["res/xml/policy.xml"] } } } },
            { "name": "sdk-b", "manifest_path": b.path().join("Cargo.toml").display().to_string(),
              "metadata": { "idealyst": { "android": { "res": ["res/xml/policy.xml"] } } } },
        ]});
        let msg = format!("{:#}", SdkManifestContributions::from_metadata(&meta).unwrap_err());
        assert!(msg.contains("sdk-a") && msg.contains("sdk-b"), "{msg}");
    }

    #[test]
    fn nothing_declared_stages_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let c = SdkManifestContributions::from_metadata(&json!({ "packages": [] })).unwrap();
        assert_eq!(c.render_application("x"), "");
        assert!(!c.stage_res(&tmp.path().join("r")).unwrap());
    }
}
