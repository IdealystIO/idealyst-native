//! Locating and downloading the AndroidX / Kotlin libraries an APK links.
//!
//! Two kinds of request reach this module, and neither ever asks a
//! repository for "the newest version":
//!
//! - **Pinned** ([`Pin`]): the framework's own closure, fixed to an exact
//!   version, file extension and SHA-1 (`PINNED_ANDROIDX` in
//!   `kotlin_runtime.rs`). [`fetch_pinned`] looks for exactly that file in
//!   the gradle cache and the idealyst cache, else downloads exactly that
//!   file and refuses it unless its SHA-1 is the pinned one. No metadata is
//!   read: the pinned set is already a complete closure.
//! - **Declared** (an SDK's `androidx = [["group", "artifact", "version"]]`):
//!   [`resolve_graph`] walks the declared coordinates' published metadata to
//!   their transitive runtime closure, at the versions that metadata asks
//!   for (highest requested wins, the rule Gradle applies). A dependency on
//!   a pinned library always gets the pin.
//!
//! # Gradle module metadata
//!
//! Since lifecycle 2.8, savedstate 1.3, collection 1.4, annotation 1.8 and
//! navigationevent, AndroidX libraries are Kotlin-multiplatform. The base
//! coordinate is an *umbrella*: its POM declares `aar` packaging, but the
//! `.aar` holds only an `AndroidManifest.xml`, and the Gradle module
//! metadata (`<artifact>-<version>.module`) sends an Android consumer to
//! the `-android` (or `-jvm`) variant through `available-at`. That variant's
//! own `.module` lists the real binary and its dependencies, which the POM
//! of the umbrella does not. Reading POMs alone therefore fetches an empty
//! AAR and misses whole libraries (`androidx.navigationevent`, which
//! `ComponentActivity` 1.12 implements).
//!
//! [`Resolver::node`] follows Gradle's rule: a POM carrying the
//! `published-with-gradle-metadata` marker means "read the `.module`". From
//! the `.module` it picks the variant an Android app's runtime classpath
//! would ([`select_android_variant`]): a `library` of usage `java-runtime`
//! (else `java-api`) for platform `androidJvm`, else `jvm`, else one with no
//! Kotlin platform (a plain Android/Java library). An `available-at` on
//! that variant is followed to the coordinate it names; otherwise the
//! variant's `files` give the binary (verified against the SHA-1 the
//! `.module` states as well as the repository's `.sha1`) and its
//! `dependencies` the next edges. Without a `.module`, the POM's
//! `<packaging>` and `<dependencies>` (compile/runtime scope, not optional)
//! are used.
//!
//! Every file downloaded — metadata included — must match the repository's
//! `.sha1`, and is stored in the idealyst cache in the gradle cache's own
//! layout (`<group>/<artifact>/<version>/<sha1>/<file>`), so later builds
//! are offline and both caches are read by the same lookup.
//!
//! # Cache locations
//!
//! - idealyst cache: `~/.idealyst/maven`, or `$IDEALYST_MAVEN_CACHE`.
//! - gradle cache: `~/.gradle/caches/modules-2/files-2.1` (Android Studio
//!   fills it). `IDEALYST_NO_GRADLE_CACHE=1` ignores it, so a build can be
//!   made to behave like one on a machine without Android Studio.
//!
//! HTTP goes through `curl`, like the rest of the CLI tooling: the tools link
//! no TLS stack, and curl ships with macOS, Windows 10+ and every Linux the
//! framework targets.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

/// Repositories tried in order. AndroidX lives on Google's repository;
/// `org.jetbrains.kotlinx` (coroutines) and Guava live on Maven Central.
pub const REPOSITORIES: &[&str] = &[
    "https://dl.google.com/android/maven2",
    "https://repo1.maven.org/maven2",
];

/// Overrides the idealyst cache directory (default `~/.idealyst/maven`).
pub const MAVEN_CACHE_ENV: &str = "IDEALYST_MAVEN_CACHE";

/// When set to anything but `0`/empty, the gradle cache is not consulted.
pub const NO_GRADLE_CACHE_ENV: &str = "IDEALYST_NO_GRADLE_CACHE";

/// One HTTP GET to a file. `Ok(false)` means the server has no such file
/// (403/404 — a static bucket answers 403 for a missing key); any other
/// failure is an error.
pub trait Fetch {
    fn fetch(&self, url: &str, dest: &Path) -> Result<bool>;
}

/// [`Fetch`] through the `curl` binary.
pub struct Curl;

impl Fetch for Curl {
    fn fetch(&self, url: &str, dest: &Path) -> Result<bool> {
        let out = Command::new("curl")
            .args(["-sSL", "--retry", "2", "-o"])
            .arg(dest)
            .args(["-w", "%{http_code}", url])
            .output()
            .context("running curl to download an Android library — is curl on PATH?")?;
        if !out.status.success() {
            let _ = fs::remove_file(dest);
            bail!("could not download {url}: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        match String::from_utf8_lossy(&out.stdout).trim() {
            "200" => Ok(true),
            "403" | "404" => {
                // curl wrote the error page to `dest`.
                let _ = fs::remove_file(dest);
                Ok(false)
            }
            code => {
                let _ = fs::remove_file(dest);
                bail!("could not download {url}: HTTP {code}")
            }
        }
    }
}

/// The idealyst cache: `$IDEALYST_MAVEN_CACHE`, else `~/.idealyst/maven`.
pub fn idealyst_maven_cache() -> Result<PathBuf> {
    if let Some(dir) = std::env::var_os(MAVEN_CACHE_ENV).filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("$HOME is not set"))?;
    Ok(PathBuf::from(home).join(".idealyst/maven"))
}

/// `~/.gradle/caches/modules-2/files-2.1` when it exists and
/// `$IDEALYST_NO_GRADLE_CACHE` doesn't switch it off.
pub fn gradle_cache_dir() -> Option<PathBuf> {
    let disabled = std::env::var(NO_GRADLE_CACHE_ENV).is_ok_and(|v| !v.is_empty() && v != "0");
    if disabled {
        return None;
    }
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home).join(".gradle/caches/modules-2/files-2.1");
    p.is_dir().then_some(p)
}

/// Where artifacts are looked for before the network, in order.
pub struct Caches {
    /// The gradle cache, when present and not disabled. Read-only.
    pub gradle: Option<PathBuf>,
    /// The idealyst cache. Downloads land here.
    pub idealyst: PathBuf,
}

impl Caches {
    /// `<cache>/<group>/<artifact>/<version>/<any hash>/<file_name>` in the
    /// gradle cache, then the idealyst cache.
    fn find(&self, coord: &Coord, file_name: &str) -> Option<PathBuf> {
        self.gradle.iter().chain(std::iter::once(&self.idealyst)).find_map(|root| {
            let dir = root.join(&coord.group).join(&coord.artifact).join(&coord.version);
            let mut hits: Vec<PathBuf> = fs::read_dir(&dir)
                .ok()?
                .filter_map(|e| e.ok())
                .map(|e| e.path().join(file_name))
                .filter(|p| p.is_file())
                .collect();
            // Two hash dirs for one file name means two different bytes for
            // one coordinate; take a stable one.
            hits.sort();
            hits.into_iter().next()
        })
    }
}

/// A Maven coordinate at an exact version.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Coord {
    pub group: String,
    pub artifact: String,
    pub version: String,
}

impl Coord {
    pub fn new(group: &str, artifact: &str, version: &str) -> Self {
        Coord { group: group.into(), artifact: artifact.into(), version: version.into() }
    }

    /// `<group as path>/<artifact>/<version>/` under `repo`.
    fn dir_url(&self, repo: &str) -> String {
        format!("{repo}/{}/{}/{}", self.group.replace('.', "/"), self.artifact, self.version)
    }

    fn file_stem(&self) -> String {
        format!("{}-{}", self.artifact, self.version)
    }
}

impl std::fmt::Display for Coord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.group, self.artifact, self.version)
    }
}

/// One library of the framework's pinned closure: an exact file, identified
/// by its SHA-1.
#[derive(Clone, Copy, Debug)]
pub struct Pin {
    pub group: &'static str,
    pub artifact: &'static str,
    pub version: &'static str,
    /// `"aar"` or `"jar"`.
    pub ext: &'static str,
    /// SHA-1 of the file — also the gradle cache's directory name for it.
    pub sha1: &'static str,
}

impl Pin {
    pub fn coord(&self) -> Coord {
        Coord::new(self.group, self.artifact, self.version)
    }

    fn file_name(&self) -> String {
        format!("{}-{}.{}", self.artifact, self.version, self.ext)
    }

    /// Whether a request for `group:artifact` is satisfied by this pin. A
    /// Kotlin-multiplatform library is published as an umbrella (`collection`)
    /// plus platform variants (`collection-jvm`, `lifecycle-runtime-android`);
    /// all of them name the same library, so a pin of any one answers for all.
    pub fn answers(&self, group: &str, artifact: &str) -> bool {
        self.group == group && library_name(self.artifact) == library_name(artifact)
    }
}

/// `artifact` without a `-jvm` / `-android` platform suffix.
pub fn library_name(artifact: &str) -> &str {
    artifact
        .strip_suffix("-android")
        .or_else(|| artifact.strip_suffix("-jvm"))
        .unwrap_or(artifact)
}

/// The pin answering `group:artifact`, if any.
pub fn pin_for<'p>(pins: &'p [Pin], group: &str, artifact: &str) -> Option<&'p Pin> {
    pins.iter().find(|p| p.answers(group, artifact))
}

/// `pin`'s file: from the gradle cache or the idealyst cache at its exact
/// `<version>/<sha1>/` path, else downloaded from `repos` and accepted only
/// if its SHA-1 is the pinned one. A cache hit never touches the network.
pub fn fetch_pinned(fetcher: &dyn Fetch, repos: &[&str], caches: &Caches, pin: &Pin) -> Result<PathBuf> {
    let coord = pin.coord();
    let file_name = pin.file_name();
    for root in caches.gradle.iter().chain(std::iter::once(&caches.idealyst)) {
        let p = root
            .join(pin.group)
            .join(pin.artifact)
            .join(pin.version)
            .join(pin.sha1)
            .join(&file_name);
        if p.is_file() {
            return Ok(p);
        }
    }
    let mut tried = Vec::new();
    for repo in repos {
        let url = format!("{}/{file_name}", coord.dir_url(repo));
        match download_verified(fetcher, &url, &caches.idealyst, &coord, &file_name, Some(pin.sha1)) {
            Ok(Some(p)) => return Ok(p),
            Ok(None) => tried.push(format!("{repo}: not published there")),
            Err(e) => tried.push(format!("{repo}: {e:#}")),
        }
    }
    bail!("could not download {coord} ({file_name}) — {}", tried.join("; "))
}

/// The SHA-1 a repository's `.sha1` file states. Accepts both the bare
/// digest and the `<digest>  <filename>` form.
pub fn parse_sha1_file(text: &str) -> Option<String> {
    let digest = text.split_whitespace().next()?.to_ascii_lowercase();
    (digest.len() == 40 && digest.chars().all(|c| c.is_ascii_hexdigit())).then_some(digest)
}

fn partial_path(cache_root: &Path, file_name: &str) -> Result<PathBuf> {
    let partial = cache_root.join(".partial");
    fs::create_dir_all(&partial).with_context(|| format!("create {}", partial.display()))?;
    Ok(partial.join(format!("{}-{file_name}", std::process::id())))
}

/// Download `url`, check it against the repository's `<url>.sha1` (and
/// `expected_sha1` when the caller knows it), and store it as
/// `<cache_root>/<group>/<artifact>/<version>/<sha1>/<file_name>`.
/// `Ok(None)` when the repository has no such file.
fn download_verified(
    fetcher: &dyn Fetch,
    url: &str,
    cache_root: &Path,
    coord: &Coord,
    file_name: &str,
    expected_sha1: Option<&str>,
) -> Result<Option<PathBuf>> {
    let download = partial_path(cache_root, file_name)?;
    if !fetcher.fetch(url, &download)? {
        return Ok(None);
    }
    let bytes = fs::read(&download).with_context(|| format!("read {}", download.display()))?;
    let digest = match verify(fetcher, url, &bytes, cache_root, expected_sha1) {
        Ok(d) => d,
        Err(e) => {
            let _ = fs::remove_file(&download);
            return Err(e);
        }
    };
    let dest_dir = cache_root
        .join(&coord.group)
        .join(&coord.artifact)
        .join(&coord.version)
        .join(&digest);
    fs::create_dir_all(&dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;
    let dest = dest_dir.join(file_name);
    // Rename within one filesystem: a crash mid-download leaves only a
    // `.partial` file, never a truncated artifact a cache lookup would find.
    fs::rename(&download, &dest)
        .with_context(|| format!("move {} to {}", download.display(), dest.display()))?;
    Ok(Some(dest))
}

/// Check a download is non-empty, matches the repository's `.sha1`, and
/// matches `expected` when given. Returns the digest, which names the cache
/// directory.
fn verify(
    fetcher: &dyn Fetch,
    url: &str,
    bytes: &[u8],
    cache_root: &Path,
    expected: Option<&str>,
) -> Result<String> {
    if bytes.is_empty() {
        bail!("{url} downloaded empty");
    }
    let sha_url = format!("{url}.sha1");
    let sha_tmp = partial_path(cache_root, "sha1.txt")?;
    if !fetcher.fetch(&sha_url, &sha_tmp)? {
        bail!("{sha_url} is missing; refusing an unverified download");
    }
    let stated = fs::read_to_string(&sha_tmp).with_context(|| format!("read {sha_url}"))?;
    let _ = fs::remove_file(&sha_tmp);
    let stated = parse_sha1_file(&stated)
        .with_context(|| format!("{sha_url} holds no SHA-1 digest: {stated:?}"))?;
    let actual = sha1_smol::Sha1::from(bytes).digest().to_string();
    if actual != stated {
        bail!("{url} failed its checksum: expected sha1 {stated}, got {actual}");
    }
    if let Some(expected) = expected {
        if actual != expected {
            bail!("{url} is not the pinned file: expected sha1 {expected}, got {actual}");
        }
    }
    Ok(actual)
}

// ── Metadata ────────────────────────────────────────────────────────────

/// A dependency edge read from a `.module` variant or a POM.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dependency {
    pub group: String,
    pub artifact: String,
    /// Empty when the publisher left the version to a BOM or parent POM
    /// (`kotlin-stdlib` often is). Fine for a provided or pinned library;
    /// [`resolve_graph`] refuses it for anything else.
    pub version: String,
}

/// A file a `.module` variant publishes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VariantFile {
    pub name: String,
    /// Relative to the `.module`'s directory.
    pub url: String,
    pub sha1: Option<String>,
}

/// What a `.module` says an Android consumer gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModuleVariant {
    /// The variant lives at another coordinate (`available-at`).
    Redirect(Coord),
    /// The variant is here: its files and dependencies.
    Local { files: Vec<VariantFile>, dependencies: Vec<Dependency> },
}

/// Gradle's marker in a POM whose `.module` should be read instead.
const MODULE_MARKER: &str = "published-with-gradle-metadata";

/// True when a POM tells metadata-aware consumers to read the `.module`.
pub fn pom_defers_to_module(pom: &str) -> bool {
    pom.contains(MODULE_MARKER)
}

/// Kotlin platforms an Android app can run, best first. `None` (no
/// `org.jetbrains.kotlin.platform.type`) is a plain Android/Java library.
const PLATFORM_RANK: &[Option<&str>] = &[Some("androidJvm"), Some("jvm"), None];

/// The variant of a parsed `.module` that an Android app's runtime
/// classpath resolves to, as Gradle selects it: category `library` (or
/// none), usage `java-runtime` preferred over `java-api`, platform per
/// [`PLATFORM_RANK`]. Documentation/sources, Kotlin metadata and native/JS
/// variants never match. Ties keep the first variant listed.
pub fn select_android_variant(module: &Value) -> Option<&Value> {
    let variants = module.get("variants")?.as_array()?;
    let mut best: Option<((usize, usize), &Value)> = None;
    for variant in variants {
        let attrs = variant.get("attributes");
        let attr = |k: &str| attrs.and_then(|a| a.get(k)).and_then(|v| v.as_str());
        if attr("org.gradle.category").is_some_and(|c| c != "library") {
            continue;
        }
        let usage_rank = match attr("org.gradle.usage") {
            Some("java-runtime") => 0,
            Some("java-api") => 1,
            _ => continue,
        };
        let Some(platform_rank) =
            PLATFORM_RANK.iter().position(|p| *p == attr("org.jetbrains.kotlin.platform.type"))
        else {
            continue;
        };
        let rank = (platform_rank, usage_rank);
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, variant));
        }
    }
    best.map(|(_, v)| v)
}

/// Parse a `.module` file into what an Android consumer gets from it.
pub fn parse_module(json: &str) -> Result<ModuleVariant> {
    let module: Value = serde_json::from_str(json).context("parse Gradle module metadata")?;
    let component = module.get("component");
    let name = format!(
        "{}:{}:{}",
        component.and_then(|c| c.get("group")).and_then(|v| v.as_str()).unwrap_or("?"),
        component.and_then(|c| c.get("module")).and_then(|v| v.as_str()).unwrap_or("?"),
        component.and_then(|c| c.get("version")).and_then(|v| v.as_str()).unwrap_or("?"),
    );
    let variant = select_android_variant(&module)
        .with_context(|| format!("{name} publishes no variant an Android app can use"))?;
    if let Some(at) = variant.get("available-at") {
        let field = |k: &str| {
            at.get(k)
                .and_then(|v| v.as_str())
                .map(str::to_string)
                .with_context(|| format!("{name}: available-at without `{k}`"))
        };
        return Ok(ModuleVariant::Redirect(Coord {
            group: field("group")?,
            artifact: field("module")?,
            version: field("version")?,
        }));
    }
    let mut dependencies = Vec::new();
    for dep in variant.get("dependencies").and_then(|d| d.as_array()).into_iter().flatten() {
        // A BOM (`platform(...)`) only constrains versions; it has no classes.
        let category = dep.pointer("/attributes/org.gradle.category").and_then(|v| v.as_str());
        if matches!(category, Some("platform" | "enforced-platform")) {
            continue;
        }
        let field = |k: &str| dep.get(k).and_then(|v| v.as_str()).map(str::to_string);
        let (Some(group), Some(artifact)) = (field("group"), field("module")) else {
            bail!("{name}: a dependency without group/module");
        };
        let version = dep
            .get("version")
            .and_then(|v| {
                ["strictly", "requires", "prefers"]
                    .iter()
                    .find_map(|k| v.get(*k).and_then(|s| s.as_str()))
            })
            .map(normalize_version_spec)
            .unwrap_or_default();
        dependencies.push(Dependency { group, artifact, version });
    }
    let files = variant
        .get("files")
        .and_then(|f| f.as_array())
        .into_iter()
        .flatten()
        .filter_map(|f| {
            let name = f.get("name")?.as_str()?.to_string();
            let url = f.get("url").and_then(|u| u.as_str()).unwrap_or(&name).to_string();
            let sha1 = f.get("sha1").and_then(|s| s.as_str()).map(str::to_string);
            Some(VariantFile { name, url, sha1 })
        })
        .collect();
    Ok(ModuleVariant::Local { files, dependencies })
}

/// A Maven version or range, as one exact version: `[2.6.2]` → `2.6.2`; a
/// range takes its lower bound (`[1.0,2.0)` → `1.0`), or its upper bound
/// when it has no lower (`(,1.5]` → `1.5`). Deterministic, and the version
/// the publisher tested against.
pub fn normalize_version_spec(spec: &str) -> String {
    let s = spec.trim();
    if !s.starts_with(['[', '(']) {
        return s.to_string();
    }
    let inner = s.trim_start_matches(['[', '(']).trim_end_matches([']', ')']);
    let mut bounds = inner.split(',').map(str::trim);
    let lower = bounds.next().unwrap_or("");
    if !lower.is_empty() {
        return lower.to_string();
    }
    bounds.next().unwrap_or("").to_string()
}

/// What a POM contributes when there is no `.module` to read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pom {
    /// `<packaging>`; absent means `jar`.
    pub packaging: String,
    pub dependencies: Vec<Dependency>,
}

/// Parse a POM: its packaging and the `<dependencies>` an app's runtime
/// classpath gets (scope compile/runtime, not optional). Versions may use
/// `${project.version}` or the POM's own `<properties>`; a version the POM
/// leaves to a parent POM or BOM is left empty (see [`Dependency::version`]).
pub fn parse_pom(xml: &str) -> Result<Pom> {
    let doc = roxmltree::Document::parse(xml).context("parse POM")?;
    let project = doc.root_element();
    fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
        node.children().find(|c| c.is_element() && c.tag_name().name() == name)
    }
    fn text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
        child(node, name).and_then(|c| c.text()).map(|t| t.trim().to_string())
    }
    let project_version = text(project, "version")
        .or_else(|| child(project, "parent").and_then(|p| text(p, "version")));
    let mut properties: HashMap<String, String> = HashMap::new();
    if let Some(props) = child(project, "properties") {
        for p in props.children().filter(|c| c.is_element()) {
            properties.insert(p.tag_name().name().to_string(), p.text().unwrap_or("").trim().to_string());
        }
    }
    let substitute = |raw: &str| -> Option<String> {
        let Some(key) = raw.strip_prefix("${").and_then(|r| r.strip_suffix('}')) else {
            return Some(raw.to_string());
        };
        match key {
            "project.version" | "pom.version" | "version" => project_version.clone(),
            other => properties.get(other).cloned(),
        }
    };
    let packaging = text(project, "packaging").unwrap_or_else(|| "jar".to_string());
    let mut dependencies = Vec::new();
    if let Some(deps) = child(project, "dependencies") {
        for dep in deps.children().filter(|c| c.is_element() && c.tag_name().name() == "dependency") {
            let scope = text(dep, "scope").unwrap_or_else(|| "compile".to_string());
            if !matches!(scope.as_str(), "compile" | "runtime") {
                continue;
            }
            if text(dep, "optional").as_deref() == Some("true") {
                continue;
            }
            let (Some(group), Some(artifact)) = (text(dep, "groupId"), text(dep, "artifactId")) else {
                bail!("POM dependency without groupId/artifactId");
            };
            let version = match text(dep, "version") {
                None => String::new(),
                Some(raw) => substitute(&raw).map(|v| normalize_version_spec(&v)).with_context(|| {
                    format!("POM dependency {group}:{artifact}: unresolvable version {raw}")
                })?,
            };
            dependencies.push(Dependency { group, artifact, version });
        }
    }
    Ok(Pom { packaging, dependencies })
}

/// Order two versions: numeric dot-parts compared as numbers, then the full
/// string (so `1.10.0` > `1.9.0`, and the order is total).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let numeric = |s: &str| -> Vec<u64> {
        s.split(['.', '-'])
            .map_while(|p| p.parse::<u64>().ok())
            .collect()
    };
    numeric(a).cmp(&numeric(b)).then_with(|| a.cmp(b))
}

// ── Resolving declared coordinates ──────────────────────────────────────

/// One resolved coordinate: what an Android app gets for it.
#[derive(Clone, Debug)]
pub struct Node {
    /// The coordinate whose files were taken — after `available-at`
    /// redirects, so the umbrella `lifecycle-runtime` resolves to
    /// `lifecycle-runtime-android`.
    pub coord: Coord,
    /// The `.aar`/`.jar`, or `None` when the variant publishes none (a
    /// BOM, a metadata-only module).
    pub binary: Option<PathBuf>,
    pub dependencies: Vec<Dependency>,
}

/// Reads metadata and binaries for exact coordinates, from the caches or
/// `repos`, memoized for one build.
pub struct Resolver<'a> {
    pub fetcher: &'a dyn Fetch,
    pub repos: &'a [&'a str],
    pub caches: &'a Caches,
    memo: RefCell<HashMap<Coord, Node>>,
}

/// `available-at` chains longer than this are refused (a cycle).
const MAX_REDIRECTS: usize = 4;

impl<'a> Resolver<'a> {
    pub fn new(fetcher: &'a dyn Fetch, repos: &'a [&'a str], caches: &'a Caches) -> Self {
        Resolver { fetcher, repos, caches, memo: RefCell::new(HashMap::new()) }
    }

    /// `file_name` of `coord`: cached, else the first repository that has it
    /// (downloaded and verified). `Ok(None)` when no repository has it.
    fn file(&self, coord: &Coord, file_name: &str, url_name: &str, sha1: Option<&str>) -> Result<Option<PathBuf>> {
        if let Some(p) = self.caches.find(coord, file_name) {
            return Ok(Some(p));
        }
        let mut errors = Vec::new();
        for repo in self.repos {
            let url = format!("{}/{url_name}", coord.dir_url(repo));
            match download_verified(self.fetcher, &url, &self.caches.idealyst, coord, file_name, sha1) {
                Ok(Some(p)) => return Ok(Some(p)),
                Ok(None) => {}
                Err(e) => errors.push(format!("{repo}: {e:#}")),
            }
        }
        if errors.is_empty() {
            Ok(None)
        } else {
            bail!("{coord} {file_name}: {}", errors.join("; "))
        }
    }

    fn text(&self, coord: &Coord, ext: &str) -> Result<Option<String>> {
        let name = format!("{}.{ext}", coord.file_stem());
        match self.file(coord, &name, &name, None)? {
            Some(p) => Ok(Some(fs::read_to_string(&p).with_context(|| format!("read {}", p.display()))?)),
            None => Ok(None),
        }
    }

    /// What an Android app gets for `coord`, following `available-at`.
    pub fn node(&self, coord: &Coord) -> Result<Node> {
        if let Some(n) = self.memo.borrow().get(coord) {
            return Ok(n.clone());
        }
        let node = self.node_uncached(coord, 0)?;
        self.memo.borrow_mut().insert(coord.clone(), node.clone());
        Ok(node)
    }

    fn node_uncached(&self, coord: &Coord, depth: usize) -> Result<Node> {
        if depth > MAX_REDIRECTS {
            bail!("{coord}: more than {MAX_REDIRECTS} available-at redirects");
        }
        let pom = self
            .text(coord, "pom")?
            .with_context(|| format!("{coord} is published in none of {}", self.repos.join(", ")))?;
        if pom_defers_to_module(&pom) {
            if let Some(module) = self.text(coord, "module")? {
                return match parse_module(&module).with_context(|| format!("{coord}.module"))? {
                    ModuleVariant::Redirect(to) => self.node_uncached(&to, depth + 1),
                    ModuleVariant::Local { files, dependencies } => {
                        let binary = match files
                            .iter()
                            .find(|f| f.name.ends_with(".aar") || f.name.ends_with(".jar"))
                        {
                            Some(f) => Some(
                                self.file(coord, &f.name, &f.url, f.sha1.as_deref())?.with_context(|| {
                                    format!("{coord}.module lists {} but the repository doesn't serve it", f.name)
                                })?,
                            ),
                            None => None,
                        };
                        Ok(Node { coord: coord.clone(), binary, dependencies })
                    }
                };
            }
        }
        let parsed = parse_pom(&pom).with_context(|| format!("{coord}.pom"))?;
        let ext = match parsed.packaging.as_str() {
            "aar" => Some("aar"),
            "pom" => None,
            // `bundle` (OSGi) and anything else ship a jar.
            _ => Some("jar"),
        };
        let binary = match ext {
            Some(ext) => {
                let name = format!("{}.{ext}", coord.file_stem());
                Some(
                    self.file(coord, &name, &name, None)?
                        .with_context(|| format!("{coord}: {name} is not published"))?,
                )
            }
            None => None,
        };
        Ok(Node { coord: coord.clone(), binary, dependencies: parsed.dependencies })
    }
}

/// Dependencies an APK never takes from Maven.
///
/// - `org.jetbrains.kotlin:kotlin-stdlib*`: the Kotlin distribution's own
///   `kotlin-stdlib.jar` is compiled against and dexed (one stdlib, matching
///   the `kotlinc` that compiles the runtime).
/// - `org.jetbrains:annotations`: `@NotNull`/`@Nullable` markers. No class
///   loads them at runtime, and d8 doesn't need annotation types present.
/// - `androidx.profileinstaller:profileinstaller`: its only entry points are
///   a receiver and an initializer registered in its own manifest, which the
///   APK doesn't merge, and no class of the pinned set references it — dexing
///   it (and its `androidx.startup` closure) would add dead code.
pub fn is_provided(group: &str, artifact: &str) -> bool {
    (group == "org.jetbrains.kotlin" && artifact.starts_with("kotlin-stdlib"))
        || (group == "org.jetbrains" && artifact == "annotations")
        || (group == "androidx.profileinstaller" && artifact == "profileinstaller")
}

/// The outcome of [`resolve_graph`].
#[derive(Debug, Default)]
pub struct Resolution {
    /// Every unpinned library the roots need, one per (group, library), in
    /// coordinate order. Nodes without a binary are kept out.
    pub nodes: Vec<Node>,
    /// Requests for a pinned library at a higher version than the pin: the
    /// pin is used, and the caller should say so.
    pub pin_overrides: Vec<String>,
}

/// The transitive runtime closure of `roots` (exact coordinates an SDK
/// declared), excluding pinned and [`is_provided`] libraries.
///
/// Versions are chosen the way Gradle chooses them: every request for a
/// library is recorded, and the highest wins. A request that raises the
/// version of a library already visited reruns the walk, so the result is
/// the fixpoint — independent of traversal order, and of what any
/// repository published since (only versions named in metadata are used).
pub fn resolve_graph(resolver: &Resolver, pins: &[Pin], roots: &[Coord]) -> Result<Resolution> {
    type Key = (String, String);
    let key = |g: &str, a: &str| -> Key { (g.to_string(), a.to_string()) };
    let mut chosen: BTreeMap<Key, String> = BTreeMap::new();
    let raise = |chosen: &mut BTreeMap<Key, String>, k: Key, v: &str| -> bool {
        match chosen.get(&k) {
            Some(cur) if compare_versions(cur, v) != Ordering::Less => false,
            _ => {
                chosen.insert(k, v.to_string());
                true
            }
        }
    };
    for r in roots {
        raise(&mut chosen, key(&r.group, &r.artifact), &r.version);
    }
    loop {
        let mut rerun = false;
        let mut visited: BTreeSet<Key> = BTreeSet::new();
        let mut nodes: BTreeMap<Key, Node> = BTreeMap::new();
        let mut overrides: BTreeSet<String> = BTreeSet::new();
        let mut queue: VecDeque<Key> =
            roots.iter().map(|r| key(&r.group, &r.artifact)).collect();
        while let Some(k) = queue.pop_front() {
            if !visited.insert(k.clone()) {
                continue;
            }
            let version = chosen[&k].clone();
            let node = resolver.node(&Coord::new(&k.0, &k.1, &version))?;
            for dep in &node.dependencies {
                if is_provided(&dep.group, &dep.artifact) {
                    continue;
                }
                if let Some(pin) = pin_for(pins, &dep.group, &dep.artifact) {
                    if compare_versions(&dep.version, pin.version) == Ordering::Greater {
                        overrides.insert(format!(
                            "{} asks for {}:{}:{}; the framework pins {}",
                            node.coord, dep.group, dep.artifact, dep.version, pin.coord()
                        ));
                    }
                    continue;
                }
                if dep.version.is_empty() {
                    bail!(
                        "{} depends on {}:{} without a version (it comes from a BOM or parent \
                         POM, which isn't read); declare that library with a version too",
                        node.coord, dep.group, dep.artifact
                    );
                }
                let dk = key(&dep.group, &dep.artifact);
                if raise(&mut chosen, dk.clone(), &dep.version) && visited.contains(&dk) {
                    rerun = true;
                }
                queue.push_back(dk);
            }
            if node.binary.is_some() {
                // An umbrella and its platform variant are one library: keep
                // the highest version reached.
                let lib = key(&node.coord.group, library_name(&node.coord.artifact));
                let keep = nodes
                    .get(&lib)
                    .is_none_or(|n| compare_versions(&node.coord.version, &n.coord.version) == Ordering::Greater);
                if keep {
                    nodes.insert(lib, node);
                }
            }
        }
        if !rerun {
            return Ok(Resolution {
                nodes: nodes.into_values().collect(),
                pin_overrides: overrides.into_iter().collect(),
            });
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// In-memory repository: URL → body. Records every URL requested.
    #[derive(Default)]
    pub struct FakeRepo {
        pub files: HashMap<String, Vec<u8>>,
        pub requested: RefCell<Vec<String>>,
    }

    impl FakeRepo {
        /// Serve `body` at `url`, with a correct `<url>.sha1` beside it.
        pub fn serve(&mut self, url: String, body: &[u8]) {
            self.files.insert(format!("{url}.sha1"), sha1_hex(body).into_bytes());
            self.files.insert(url, body.to_vec());
        }
    }

    impl Fetch for FakeRepo {
        fn fetch(&self, url: &str, dest: &Path) -> Result<bool> {
            self.requested.borrow_mut().push(url.to_string());
            match self.files.get(url) {
                Some(body) => {
                    fs::write(dest, body)?;
                    Ok(true)
                }
                None => Ok(false),
            }
        }
    }

    /// A fetcher that fails the test if anything reaches the network.
    pub struct NoNetwork;
    impl Fetch for NoNetwork {
        fn fetch(&self, url: &str, _dest: &Path) -> Result<bool> {
            panic!("unexpected network request for {url}");
        }
    }

    pub const REPO: &str = "https://repo.test/maven2";

    pub fn sha1_hex(bytes: &[u8]) -> String {
        sha1_smol::Sha1::from(bytes).digest().to_string()
    }

    pub fn base_url(c: &Coord) -> String {
        format!("{}/{}", c.dir_url(REPO), c.file_stem())
    }

    /// A POM for `c` with `packaging` and `deps`, optionally carrying the
    /// Gradle-metadata marker.
    pub fn pom(c: &Coord, packaging: &str, deps: &[(&str, &str, &str)], marker: bool) -> String {
        let deps: String = deps
            .iter()
            .map(|(g, a, v)| {
                format!("<dependency><groupId>{g}</groupId><artifactId>{a}</artifactId><version>{v}</version><scope>runtime</scope></dependency>")
            })
            .collect();
        let marker = if marker { format!("<!-- do_not_remove: {MODULE_MARKER} -->") } else { String::new() };
        format!(
            "<?xml version=\"1.0\"?><project xmlns=\"http://maven.apache.org/POM/4.0.0\">{marker}\
             <groupId>{}</groupId><artifactId>{}</artifactId><version>{}</version>\
             <packaging>{packaging}</packaging><dependencies>{deps}</dependencies></project>",
            c.group, c.artifact, c.version
        )
    }

    /// A KMP umbrella `.module` whose Android runtime variant lives at `to`
    /// (plus the common/native/desktop variants an Android consumer must
    /// skip).
    pub fn umbrella_module(c: &Coord, to: &Coord) -> String {
        serde_json::json!({
            "formatVersion": "1.1",
            "component": {"group": c.group, "module": c.artifact, "version": c.version},
            "variants": [
                {"name": "metadataApiElements",
                 "attributes": {"org.gradle.category": "library", "org.gradle.usage": "kotlin-metadata",
                                "org.jetbrains.kotlin.platform.type": "common"},
                 "dependencies": [{"group": "com.example", "module": "common-only", "version": {"requires": "9.0"}}],
                 "files": [{"name": format!("{}-metadata-{}.jar", c.artifact, c.version), "url": "x.jar"}]},
                {"name": "desktopRuntimeElements-published",
                 "attributes": {"org.gradle.category": "library", "org.gradle.usage": "java-runtime",
                                "org.jetbrains.kotlin.platform.type": "jvm"},
                 "available-at": {"url": "../../desktop", "group": c.group, "module": format!("{}-desktop", c.artifact), "version": c.version}},
                {"name": "iosArm64ApiElements-published",
                 "attributes": {"org.gradle.category": "library", "org.gradle.usage": "kotlin-api",
                                "org.jetbrains.kotlin.platform.type": "native"},
                 "available-at": {"url": "../../ios", "group": c.group, "module": format!("{}-iosarm64", c.artifact), "version": c.version}},
                {"name": "androidRuntimeElements-published",
                 "attributes": {"org.gradle.category": "library", "org.gradle.usage": "java-runtime",
                                "org.gradle.jvm.environment": "android",
                                "org.jetbrains.kotlin.platform.type": "androidJvm"},
                 "available-at": {"url": "../../x", "group": to.group, "module": to.artifact, "version": to.version}},
                {"name": "androidSourcesElements-published",
                 "attributes": {"org.gradle.category": "documentation", "org.gradle.usage": "java-runtime",
                                "org.jetbrains.kotlin.platform.type": "androidJvm"},
                 "available-at": {"url": "../../y", "group": to.group, "module": "wrong-sources", "version": to.version}}
            ]
        })
        .to_string()
    }

    /// A platform variant's `.module`: one AAR (with its SHA-1) and the
    /// runtime dependencies, plus an API variant with fewer.
    pub fn variant_module(c: &Coord, aar: &[u8], deps: &[(&str, &str, &str)]) -> String {
        let deps: Vec<Value> = deps
            .iter()
            .map(|(g, a, v)| serde_json::json!({"group": g, "module": a, "version": {"requires": v}}))
            .collect();
        let file = serde_json::json!({"name": format!("{}.aar", c.file_stem()), "url": format!("{}.aar", c.file_stem()), "sha1": sha1_hex(aar)});
        serde_json::json!({
            "formatVersion": "1.1",
            "component": {"group": c.group, "module": c.artifact, "version": c.version},
            "variants": [
                {"name": "androidApiElements-published",
                 "attributes": {"org.gradle.category": "library", "org.gradle.usage": "java-api",
                                "org.jetbrains.kotlin.platform.type": "androidJvm"},
                 "dependencies": [], "files": [file.clone()]},
                {"name": "androidRuntimeElements-published",
                 "attributes": {"org.gradle.category": "library", "org.gradle.usage": "java-runtime",
                                "org.jetbrains.kotlin.platform.type": "androidJvm"},
                 "dependencies": deps, "files": [file]}
            ]
        })
        .to_string()
    }

    /// Publish a KMP library: umbrella `c` (POM with the marker, an AAR
    /// holding only a manifest, an umbrella `.module`) and its `-android`
    /// variant with a real AAR and `deps`.
    pub fn publish_kmp(repo: &mut FakeRepo, c: &Coord, deps: &[(&str, &str, &str)]) -> Coord {
        let variant = Coord::new(&c.group, &format!("{}-android", c.artifact), &c.version);
        let b = base_url(c);
        repo.serve(format!("{b}.pom"), pom(c, "aar", &[], true).as_bytes());
        repo.serve(format!("{b}.aar"), b"umbrella: AndroidManifest.xml only");
        repo.serve(format!("{b}.module"), umbrella_module(c, &variant).as_bytes());
        let aar = format!("classes of {variant}").into_bytes();
        let vb = base_url(&variant);
        repo.serve(format!("{vb}.pom"), pom(&variant, "aar", &[], true).as_bytes());
        repo.serve(format!("{vb}.module"), variant_module(&variant, &aar, deps).as_bytes());
        repo.serve(format!("{vb}.aar"), &aar);
        variant
    }

    /// Publish a plain (POM-only) library.
    pub fn publish_plain(repo: &mut FakeRepo, c: &Coord, ext: &str, deps: &[(&str, &str, &str)]) {
        let b = base_url(c);
        repo.serve(format!("{b}.pom"), pom(c, ext, deps, false).as_bytes());
        repo.serve(format!("{b}.{ext}"), format!("binary of {c}").as_bytes());
    }

    fn caches(dir: &Path) -> Caches {
        Caches { gradle: None, idealyst: dir.to_path_buf() }
    }

    #[test]
    fn sha1_file_parses_both_forms() {
        let d = "95994b41b2e6e8c98b56f8cf96269495cc2855b1";
        assert_eq!(parse_sha1_file(d).as_deref(), Some(d));
        assert_eq!(parse_sha1_file(&format!("{d}  foo.aar\n")).as_deref(), Some(d));
        assert_eq!(parse_sha1_file("<html>not found</html>"), None);
    }

    #[test]
    fn version_specs_normalize_to_one_exact_version() {
        assert_eq!(normalize_version_spec("2.6.2"), "2.6.2");
        assert_eq!(normalize_version_spec("[2.6.2]"), "2.6.2");
        assert_eq!(normalize_version_spec("[1.0,2.0)"), "1.0");
        assert_eq!(normalize_version_spec("(,1.5]"), "1.5");
    }

    #[test]
    fn versions_order_numerically() {
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Ordering::Greater);
        assert_eq!(compare_versions("2.6.2", "2.6.2"), Ordering::Equal);
        assert_eq!(compare_versions("1.0", "1.0.1"), Ordering::Less);
    }

    #[test]
    fn library_name_strips_platform_suffixes() {
        assert_eq!(library_name("collection-jvm"), "collection");
        assert_eq!(library_name("lifecycle-runtime-android"), "lifecycle-runtime");
        assert_eq!(library_name("core-ktx"), "core-ktx");
        let pin = Pin { group: "androidx.collection", artifact: "collection-jvm", version: "1.4.0", ext: "jar", sha1: "" };
        assert!(pin.answers("androidx.collection", "collection"));
        assert!(pin.answers("androidx.collection", "collection-android"));
        assert!(!pin.answers("androidx.collection", "collection-ktx"));
    }

    #[test]
    fn pom_dependencies_skip_test_provided_optional_and_dependency_management() {
        let xml = r#"<project xmlns="http://maven.apache.org/POM/4.0.0">
            <version>1.2.0</version>
            <properties><dep.v>3.1</dep.v></properties>
            <packaging>aar</packaging>
            <dependencyManagement><dependencies><dependency>
              <groupId>managed</groupId><artifactId>only</artifactId><version>9</version>
            </dependency></dependencies></dependencyManagement>
            <dependencies>
              <dependency><groupId>a</groupId><artifactId>compile-dep</artifactId><version>[1.0]</version></dependency>
              <dependency><groupId>a</groupId><artifactId>runtime-dep</artifactId><version>${project.version}</version><scope>runtime</scope></dependency>
              <dependency><groupId>a</groupId><artifactId>prop-dep</artifactId><version>${dep.v}</version></dependency>
              <dependency><groupId>a</groupId><artifactId>test-dep</artifactId><version>1</version><scope>test</scope></dependency>
              <dependency><groupId>a</groupId><artifactId>provided-dep</artifactId><version>1</version><scope>provided</scope></dependency>
              <dependency><groupId>a</groupId><artifactId>optional-dep</artifactId><version>1</version><optional>true</optional></dependency>
            </dependencies></project>"#;
        let pom = parse_pom(xml).unwrap();
        assert_eq!(pom.packaging, "aar");
        let names: Vec<_> = pom.dependencies.iter().map(|d| format!("{}:{}", d.artifact, d.version)).collect();
        assert_eq!(names, ["compile-dep:1.0", "runtime-dep:1.2.0", "prop-dep:3.1"]);
        let unversioned = parse_pom("<project><dependencies><dependency><groupId>a</groupId><artifactId>b</artifactId></dependency></dependencies></project>").unwrap();
        assert_eq!(unversioned.dependencies[0].version, "");
    }

    #[test]
    fn umbrella_module_sends_an_android_consumer_to_the_android_variant() {
        let c = Coord::new("androidx.lifecycle", "lifecycle-runtime", "2.11.0");
        let to = Coord::new("androidx.lifecycle", "lifecycle-runtime-android", "2.11.0");
        assert_eq!(parse_module(&umbrella_module(&c, &to)).unwrap(), ModuleVariant::Redirect(to));
    }

    #[test]
    fn jvm_only_umbrella_redirects_to_the_jvm_variant() {
        // androidx.collection 1.4.0 publishes jvm + native, no androidJvm.
        let json = r#"{"component":{"group":"androidx.collection","module":"collection","version":"1.4.0"},
          "variants":[
            {"name":"metadataApiElements","attributes":{"org.gradle.category":"library","org.gradle.usage":"kotlin-metadata","org.jetbrains.kotlin.platform.type":"common"}},
            {"name":"iosArm64ApiElements-published","attributes":{"org.gradle.category":"library","org.gradle.usage":"kotlin-api","org.jetbrains.kotlin.platform.type":"native"},
             "available-at":{"group":"androidx.collection","module":"collection-iosarm64","version":"1.4.0"}},
            {"name":"jvmApiElements-published","attributes":{"org.gradle.category":"library","org.gradle.usage":"java-api","org.jetbrains.kotlin.platform.type":"jvm"},
             "available-at":{"group":"androidx.collection","module":"collection-jvm","version":"1.4.0"}},
            {"name":"jvmRuntimeElements-published","attributes":{"org.gradle.category":"library","org.gradle.usage":"java-runtime","org.jetbrains.kotlin.platform.type":"jvm"},
             "available-at":{"group":"androidx.collection","module":"collection-jvm","version":"1.4.0"}}]}"#;
        assert_eq!(
            parse_module(json).unwrap(),
            ModuleVariant::Redirect(Coord::new("androidx.collection", "collection-jvm", "1.4.0"))
        );
    }

    #[test]
    fn plain_android_module_takes_the_runtime_variant_and_drops_boms() {
        // The shape of androidx.activity 1.8.2 / kotlinx-coroutines-android.
        let json = r#"{"component":{"group":"g","module":"m","version":"1"},"variants":[
          {"name":"releaseVariantReleaseApiPublication","attributes":{"org.gradle.category":"library","org.gradle.usage":"java-api"},
           "dependencies":[{"group":"g","module":"api-only","version":{"requires":"1"}}],
           "files":[{"name":"m-1.aar","url":"m-1.aar","sha1":"aa"}]},
          {"name":"releaseVariantReleaseRuntimePublication","attributes":{"org.gradle.category":"library","org.gradle.usage":"java-runtime"},
           "dependencies":[{"group":"g","module":"rt","version":{"requires":"2","prefers":"3"}},
                           {"group":"g","module":"strict","version":{"strictly":"[4]","requires":"4"}},
                           {"group":"org.jetbrains.kotlinx","module":"kotlinx-coroutines-bom","version":{"requires":"1.7.3"},
                            "attributes":{"org.gradle.category":"platform"}}],
           "files":[{"name":"m-1.aar","url":"m-1.aar","sha1":"aa"}]},
          {"name":"sourcesElements","attributes":{"org.gradle.category":"documentation","org.gradle.usage":"java-runtime"},
           "files":[{"name":"m-1-sources.jar","url":"m-1-sources.jar"}]}]}"#;
        let ModuleVariant::Local { files, dependencies } = parse_module(json).unwrap() else { panic!("redirect") };
        assert_eq!(files, [VariantFile { name: "m-1.aar".into(), url: "m-1.aar".into(), sha1: Some("aa".into()) }]);
        let deps: Vec<_> = dependencies.iter().map(|d| format!("{}:{}", d.artifact, d.version)).collect();
        assert_eq!(deps, ["rt:2", "strict:4"]);
    }

    #[test]
    fn a_module_with_no_android_variant_is_an_error() {
        let json = r#"{"component":{"group":"g","module":"js-only","version":"1"},"variants":[
          {"name":"jsApiElements","attributes":{"org.gradle.category":"library","org.gradle.usage":"kotlin-api","org.jetbrains.kotlin.platform.type":"js"}}]}"#;
        assert!(format!("{:#}", parse_module(json).unwrap_err()).contains("g:js-only:1"));
    }

    #[test]
    fn pinned_download_is_verified_against_the_pin_and_stored_in_gradle_layout() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let body = b"aar-bytes";
        let sha = sha1_hex(body);
        let pin = Pin { group: "androidx.customview", artifact: "customview-poolingcontainer", version: "1.1.0", ext: "aar", sha1: Box::leak(sha.clone().into_boxed_str()) };
        repo.serve(format!("{}.aar", base_url(&pin.coord())), body);
        let p = fetch_pinned(&repo, &[REPO], &caches(cache.path()), &pin).unwrap();
        assert_eq!(
            p,
            cache.path().join("androidx.customview/customview-poolingcontainer/1.1.0").join(&sha)
                .join("customview-poolingcontainer-1.1.0.aar")
        );
        // Exactly the pinned file was asked for: no version listing, no POM.
        assert!(repo.requested.borrow().iter().all(|u| !u.contains("maven-metadata") && !u.ends_with(".pom")));
        // A second build is offline.
        assert_eq!(fetch_pinned(&NoNetwork, &[REPO], &caches(cache.path()), &pin).unwrap(), p);
    }

    #[test]
    fn a_pinned_download_with_other_bytes_is_refused() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let pin = Pin { group: "androidx.core", artifact: "core", version: "1.12.0", ext: "aar", sha1: "5aa3088ed93ba8ebaea2b8a83befdfa829339e7a" };
        // The repository's own .sha1 agrees with the bytes; the pin doesn't.
        repo.serve(format!("{}.aar", base_url(&pin.coord())), b"republished");
        let err = format!("{:#}", fetch_pinned(&repo, &[REPO], &caches(cache.path()), &pin).unwrap_err());
        assert!(err.contains("not the pinned file"), "{err}");
        assert!(!cache.path().join("androidx.core").exists());
    }

    #[test]
    fn checksum_mismatch_and_missing_sha1_are_refused() {
        let cache = tempfile::tempdir().unwrap();
        let c = Coord::new("com.example", "lib", "1.0");
        let mut repo = FakeRepo::default();
        publish_plain(&mut repo, &c, "jar", &[]);
        repo.files.insert(format!("{}.jar", base_url(&c)), b"tampered".to_vec());
        let caches = caches(cache.path());
        let err = format!("{:#}", Resolver::new(&repo, &[REPO], &caches).node(&c).unwrap_err());
        assert!(err.contains("checksum"), "{err}");

        let mut repo = FakeRepo::default();
        publish_plain(&mut repo, &Coord::new("com.example", "other", "1.0"), "jar", &[]);
        repo.files.retain(|k, _| !k.ends_with(".sha1"));
        let err = format!(
            "{:#}",
            Resolver::new(&repo, &[REPO], &caches).node(&Coord::new("com.example", "other", "1.0")).unwrap_err()
        );
        assert!(err.contains("unverified"), "{err}");
    }

    /// Regression (CrewForge, clean Ubuntu): `lifecycle-runtime` 2.11.0's
    /// POM declares `aar` packaging, so the POM-only resolver downloaded the
    /// umbrella AAR — an `AndroidManifest.xml` and a LICENSE — and the build
    /// died unzipping its absent `classes.jar`. Following the `.module` to
    /// `lifecycle-runtime-android` takes the real classes, and the umbrella
    /// binary is never fetched.
    #[test]
    fn regression_kmp_umbrella_resolves_to_its_android_variant() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let c = Coord::new("androidx.lifecycle", "lifecycle-runtime", "2.11.0");
        let variant = publish_kmp(&mut repo, &c, &[]);
        let caches = caches(cache.path());
        let node = Resolver::new(&repo, &[REPO], &caches).node(&c).unwrap();
        assert_eq!(node.coord, variant);
        let bin = node.binary.unwrap();
        assert!(bin.ends_with("lifecycle-runtime-android-2.11.0.aar"), "{}", bin.display());
        assert_eq!(fs::read(&bin).unwrap(), format!("classes of {variant}").into_bytes());
        assert!(!repo.requested.borrow().iter().any(|u| u.ends_with("lifecycle-runtime-2.11.0.aar")));
    }

    /// Regression (CrewForge, clean Ubuntu): with the umbrellas swapped for
    /// their `-android` AARs by hand, kotlinc still failed — `cannot access
    /// 'NavigationEventDispatcherOwner'` — because `androidx.navigationevent`
    /// is declared only in the `-android` variant's `.module`, which a POM
    /// reader never sees. The walk must take the variant's dependencies,
    /// themselves umbrellas, through to their own variants.
    #[test]
    fn regression_dependencies_declared_only_by_the_variant_are_resolved() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let activity = Coord::new("com.example", "activity", "1.12.0");
        publish_kmp(&mut repo, &activity, &[("androidx.navigationevent", "navigationevent", "1.0.0")]);
        let nav = Coord::new("androidx.navigationevent", "navigationevent", "1.0.0");
        publish_kmp(&mut repo, &nav, &[("org.jetbrains.kotlin", "kotlin-stdlib", "2.0.0")]);
        let caches = caches(cache.path());
        let resolver = Resolver::new(&repo, &[REPO], &caches);
        let got = resolve_graph(&resolver, &[], &[activity]).unwrap();
        let coords: Vec<String> = got.nodes.iter().map(|n| n.coord.to_string()).collect();
        assert_eq!(
            coords,
            ["androidx.navigationevent:navigationevent-android:1.0.0", "com.example:activity-android:1.12.0"]
        );
        // The common-metadata variant's dependency never leaks in, and the
        // Kotlin stdlib comes from the Kotlin distribution.
        assert!(!repo.requested.borrow().iter().any(|u| u.contains("common-only") || u.contains("kotlin-stdlib")));
    }

    #[test]
    fn highest_requested_version_wins_and_rewalks() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        // root → a:1 → c:1 ; root → b:1 → a:2 → (nothing). a:1's dep c must
        // not survive once a:2 wins.
        publish_plain(&mut repo, &Coord::new("x", "root", "1"), "jar", &[("x", "a", "1"), ("x", "b", "1")]);
        publish_plain(&mut repo, &Coord::new("x", "a", "1"), "jar", &[("x", "c", "1")]);
        publish_plain(&mut repo, &Coord::new("x", "b", "1"), "jar", &[("x", "a", "2")]);
        publish_plain(&mut repo, &Coord::new("x", "a", "2"), "jar", &[]);
        publish_plain(&mut repo, &Coord::new("x", "c", "1"), "jar", &[]);
        let caches = caches(cache.path());
        let got = resolve_graph(&Resolver::new(&repo, &[REPO], &caches), &[], &[Coord::new("x", "root", "1")]).unwrap();
        let coords: Vec<String> = got.nodes.iter().map(|n| n.coord.to_string()).collect();
        assert_eq!(coords, ["x:a:2", "x:b:1", "x:root:1"]);
        assert!(!repo.requested.borrow().iter().any(|u| u.contains("maven-metadata")));
    }

    #[test]
    fn a_pinned_dependency_gets_the_pin_and_a_higher_request_is_reported() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let root = Coord::new("com.example", "player", "1.0");
        // `collection` (umbrella name) and `core` both pinned; the request for
        // core is above the pin.
        publish_plain(&mut repo, &root, "aar", &[("androidx.collection", "collection", "1.1.0"), ("androidx.core", "core", "1.13.0")]);
        let pins = [
            Pin { group: "androidx.collection", artifact: "collection-jvm", version: "1.4.0", ext: "jar", sha1: "" },
            Pin { group: "androidx.core", artifact: "core", version: "1.12.0", ext: "aar", sha1: "" },
        ];
        let caches = caches(cache.path());
        let got = resolve_graph(&Resolver::new(&repo, &[REPO], &caches), &pins, &[root]).unwrap();
        assert_eq!(got.nodes.len(), 1);
        assert_eq!(got.pin_overrides.len(), 1, "{:?}", got.pin_overrides);
        assert!(got.pin_overrides[0].contains("androidx.core:core:1.13.0"));
        assert!(!repo.requested.borrow().iter().any(|u| u.contains("androidx/core") || u.contains("androidx/collection")));
    }

    #[test]
    fn an_unversioned_dependency_is_fine_only_when_provided_or_pinned() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let root = Coord::new("com.example", "lib", "1.0");
        publish_plain(&mut repo, &root, "jar", &[("org.jetbrains.kotlin", "kotlin-stdlib", ""), ("com.example", "bom-managed", "")]);
        let caches = caches(cache.path());
        let err = format!("{:#}", resolve_graph(&Resolver::new(&repo, &[REPO], &caches), &[], &[root]).unwrap_err());
        assert!(err.contains("com.example:bom-managed without a version"), "{err}");
    }

    #[test]
    fn a_resolved_graph_is_offline_on_the_next_build() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let c = Coord::new("androidx.lifecycle", "lifecycle-runtime", "2.11.0");
        publish_kmp(&mut repo, &c, &[]);
        let caches = caches(cache.path());
        let first = resolve_graph(&Resolver::new(&repo, &[REPO], &caches), &[], &[c.clone()]).unwrap();
        let again = resolve_graph(&Resolver::new(&NoNetwork, &[REPO], &caches), &[], &[c]).unwrap();
        assert_eq!(first.nodes[0].binary, again.nodes[0].binary);
    }

    #[test]
    fn a_repository_without_the_artifact_falls_through_to_the_next() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let c = Coord::new("org.jetbrains.kotlinx", "kotlinx-coroutines-android", "1.7.3");
        publish_plain(&mut repo, &c, "jar", &[]);
        let caches = caches(cache.path());
        let node = Resolver::new(&repo, &["https://other.test/m2", REPO], &caches).node(&c).unwrap();
        assert!(node.binary.unwrap().ends_with("kotlinx-coroutines-android-1.7.3.jar"));
    }

    #[test]
    fn unpublished_everywhere_names_every_repository() {
        let cache = tempfile::tempdir().unwrap();
        let caches = caches(cache.path());
        let err = Resolver::new(&FakeRepo::default(), &["https://a.test", "https://b.test"], &caches)
            .node(&Coord::new("androidx.nope", "nope", "1"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("https://a.test") && err.contains("https://b.test"), "{err}");
    }

    /// Hits the real Google Maven repository. Run with `--ignored`.
    #[test]
    #[ignore = "network: downloads from dl.google.com"]
    fn live_lifecycle_runtime_2_11_resolves_to_its_android_variant() {
        let cache = tempfile::tempdir().unwrap();
        let caches = caches(cache.path());
        let resolver = Resolver::new(&Curl, REPOSITORIES, &caches);
        let node = resolver.node(&Coord::new("androidx.lifecycle", "lifecycle-runtime", "2.11.0")).unwrap();
        assert_eq!(node.coord.artifact, "lifecycle-runtime-android");
        assert!(node.binary.unwrap().extension().unwrap() == "aar");
    }
}
