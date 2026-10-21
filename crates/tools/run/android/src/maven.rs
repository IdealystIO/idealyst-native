//! Downloading AndroidX / Kotlin artifacts that neither local cache has.
//!
//! The Kotlin runtime build (`kotlin_runtime.rs`) looks in the gradle cache
//! first, then in the idealyst Maven cache this module fills. When both miss,
//! [`fetch_from_maven`] downloads the artifact from [`REPOSITORIES`] into the
//! idealyst cache using the gradle cache's own layout
//! (`<group>/<artifact>/<version>/<sha1>/<artifact>-<version>.<ext>`), so the
//! one directory walker (`find_artifact`) reads both caches.
//!
//! Version choice ([`choose_version`]): the caller's preferred version when
//! the repository publishes it, else the newest *stable* version in the
//! repository's `maven-metadata.xml` (no `-alpha`/`-rc` qualifier). The
//! caller prefers the version the rest of the artifact's group resolved to:
//! AndroidX releases each group in lockstep and splits or merges artifacts
//! across releases, so mixing versions within a group breaks at runtime —
//! `core-ktx` 1.13+ is an empty jar whose extensions moved into `core`, so
//! the newest `core-ktx` next to a cached `core` 1.12 leaves
//! `androidx.core.view.ViewGroupKt` on neither. Unlike the gradle-cache
//! walker, the download never falls back to an older version when the newest
//! has no binary: on a Kotlin-multiplatform relocation (the base coordinate's
//! Gradle `.module` sends every variant elsewhere via `available-at`) it
//! fetches `<artifact>-jvm` instead — the same `-jvm` rule the gradle-cache
//! walker applies — because an older base jar is exactly the stale-class trap
//! `kmp_relocated_modules_pin_jvm_variant` guards against.
//!
//! HTTP goes through `curl`, like the rest of the CLI tooling: the tools link
//! no TLS stack, and curl ships with macOS, Windows 10+ and every Linux the
//! framework targets.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Repositories tried in order. AndroidX lives on Google's repository;
/// `org.jetbrains.kotlinx` (coroutines) lives on Maven Central.
pub const REPOSITORIES: &[&str] = &[
    "https://dl.google.com/android/maven2",
    "https://repo1.maven.org/maven2",
];

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

/// `~/.idealyst/maven` — where downloaded artifacts are kept between builds.
pub fn idealyst_maven_cache() -> Result<PathBuf> {
    let home = std::env::var("HOME").map_err(|_| anyhow::anyhow!("$HOME is not set"))?;
    Ok(PathBuf::from(home).join(".idealyst/maven"))
}

/// `<repo>/<group as path>/<artifact>` — the directory holding
/// `maven-metadata.xml` and one subdirectory per version.
pub fn artifact_dir_url(repo: &str, group: &str, artifact: &str) -> String {
    format!("{repo}/{}/{artifact}", group.replace('.', "/"))
}

/// Every `<version>` listed under `<versions>` in a `maven-metadata.xml`.
pub fn parse_metadata_versions(xml: &str) -> Vec<String> {
    let Some(start) = xml.find("<versions>") else { return Vec::new() };
    let rest = &xml[start..];
    let block = &rest[..rest.find("</versions>").unwrap_or(rest.len())];
    block
        .split("<version>")
        .skip(1)
        .filter_map(|chunk| chunk.split_once("</version>").map(|(v, _)| v.trim().to_string()))
        .filter(|v| !v.is_empty())
        .collect()
}

/// The highest version made only of dot-separated numbers. Qualified
/// versions (`1.1.0-rc01`, `2.0.0-beta1`) are prereleases and never picked.
pub fn newest_stable(versions: &[String]) -> Option<String> {
    versions
        .iter()
        .filter_map(|v| {
            let parts: Option<Vec<u64>> = v.split('.').map(|p| p.parse::<u64>().ok()).collect();
            parts.map(|p| (p, v))
        })
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, v)| v.clone())
}

/// `preferred` when `versions` lists it, else [`newest_stable`].
pub fn choose_version(versions: &[String], preferred: Option<&str>) -> Option<String> {
    match preferred {
        Some(p) if versions.iter().any(|v| v == p) => Some(p.to_string()),
        _ => newest_stable(versions),
    }
}

/// The project's `<packaging>`, if the POM declares one (absent means jar).
/// Dependencies carry no `<packaging>`, so the first occurrence is the
/// project's own.
pub fn pom_packaging(pom: &str) -> Option<String> {
    let start = pom.find("<packaging>")? + "<packaging>".len();
    let end = pom[start..].find("</packaging>")?;
    Some(pom[start..start + end].trim().to_string())
}

/// True when a Gradle `.module` file sends its variants to other
/// coordinates (`available-at`) — a Kotlin-multiplatform root whose own
/// jar holds only metadata.
pub fn module_is_relocated(module_json: &str) -> bool {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(module_json) else { return false };
    v.get("variants")
        .and_then(|vs| vs.as_array())
        .is_some_and(|vs| vs.iter().any(|variant| variant.get("available-at").is_some()))
}

/// The SHA-1 a repository's `.sha1` file states. Accepts both the bare
/// digest and the `<digest>  <filename>` form.
pub fn parse_sha1_file(text: &str) -> Option<String> {
    let digest = text.split_whitespace().next()?.to_ascii_lowercase();
    (digest.len() == 40 && digest.chars().all(|c| c.is_ascii_hexdigit())).then_some(digest)
}

/// Download `group:artifact` ([`choose_version`] with `preferred`) into
/// `cache_root`, trying each of [`REPOSITORIES`] in turn. Returns the
/// stored `.aar`/`.jar`.
pub fn fetch_from_maven(
    fetcher: &dyn Fetch,
    cache_root: &Path,
    group: &str,
    artifact: &str,
    preferred: Option<&str>,
) -> Result<PathBuf> {
    fetch_from_repos(fetcher, REPOSITORIES, cache_root, group, artifact, preferred)
}

pub fn fetch_from_repos(
    fetcher: &dyn Fetch,
    repos: &[&str],
    cache_root: &Path,
    group: &str,
    artifact: &str,
    preferred: Option<&str>,
) -> Result<PathBuf> {
    let mut tried = Vec::new();
    for repo in repos {
        match fetch_from_repo(fetcher, repo, cache_root, group, artifact, preferred) {
            Ok(Some(p)) => return Ok(p),
            Ok(None) => tried.push(format!("{repo}: not published there")),
            Err(e) => tried.push(format!("{repo}: {e:#}")),
        }
    }
    bail!("could not download {group}:{artifact} — {}", tried.join("; "))
}

/// `Ok(None)` when the repository doesn't publish the artifact at all.
fn fetch_from_repo(
    fetcher: &dyn Fetch,
    repo: &str,
    cache_root: &Path,
    group: &str,
    artifact: &str,
    preferred: Option<&str>,
) -> Result<Option<PathBuf>> {
    let partial = cache_root.join(".partial");
    fs::create_dir_all(&partial).with_context(|| format!("create {}", partial.display()))?;
    let tmp = |suffix: &str| partial.join(format!("{artifact}-{}.{suffix}", std::process::id()));
    let get_text = |url: &str| -> Result<Option<String>> {
        let dest = tmp("txt");
        if !fetcher.fetch(url, &dest)? {
            return Ok(None);
        }
        let text = fs::read_to_string(&dest).with_context(|| format!("read {url}"))?;
        let _ = fs::remove_file(&dest);
        Ok(Some(text))
    };

    let dir_url = artifact_dir_url(repo, group, artifact);
    let Some(metadata) = get_text(&format!("{dir_url}/maven-metadata.xml"))? else {
        return Ok(None);
    };
    let versions = parse_metadata_versions(&metadata);
    let version = choose_version(&versions, preferred).with_context(|| {
        format!("{group}:{artifact} lists no stable version (versions: {})", versions.join(", "))
    })?;
    let base_url = format!("{dir_url}/{version}/{artifact}-{version}");
    let pom = get_text(&format!("{base_url}.pom"))?
        .with_context(|| format!("{base_url}.pom is missing"))?;
    let packaging = pom_packaging(&pom);

    let ext = if packaging.as_deref() == Some("aar") {
        "aar"
    } else {
        let relocated = get_text(&format!("{base_url}.module"))?
            .is_some_and(|m| module_is_relocated(&m));
        if relocated {
            if artifact.ends_with("-jvm") {
                bail!("{group}:{artifact}:{version} is relocated again — no binary to fetch");
            }
            return fetch_from_repo(fetcher, repo, cache_root, group, &format!("{artifact}-jvm"), preferred);
        }
        if packaging.as_deref() == Some("pom") {
            bail!("{group}:{artifact}:{version} publishes no binary (pom packaging)");
        }
        "jar"
    };

    let file_name = format!("{artifact}-{version}.{ext}");
    let binary_url = format!("{base_url}.{ext}");
    let download = tmp(ext);
    if !fetcher.fetch(&binary_url, &download)? {
        bail!("{binary_url} is missing");
    }
    let bytes = fs::read(&download).with_context(|| format!("read {}", download.display()))?;
    let digest = match verify(&binary_url, &bytes, &get_text) {
        Ok(d) => d,
        Err(e) => {
            let _ = fs::remove_file(&download);
            return Err(e);
        }
    };

    let dest_dir = cache_root.join(group).join(artifact).join(&version).join(&digest);
    fs::create_dir_all(&dest_dir).with_context(|| format!("create {}", dest_dir.display()))?;
    let dest = dest_dir.join(file_name);
    // Rename within one filesystem: a crash mid-download leaves only a
    // `.partial` file, never a truncated artifact the walker would pick up.
    fs::rename(&download, &dest)
        .with_context(|| format!("move {} to {}", download.display(), dest.display()))?;
    Ok(Some(dest))
}

/// Check a download is non-empty and matches the repository's `.sha1`.
/// Returns the digest, which names the cache directory.
fn verify(
    binary_url: &str,
    bytes: &[u8],
    get_text: &dyn Fn(&str) -> Result<Option<String>>,
) -> Result<String> {
    if bytes.is_empty() {
        bail!("{binary_url} downloaded empty");
    }
    let sha_url = format!("{binary_url}.sha1");
    let stated = get_text(&sha_url)?
        .with_context(|| format!("{sha_url} is missing; refusing an unverified download"))?;
    let expected = parse_sha1_file(&stated)
        .with_context(|| format!("{sha_url} holds no SHA-1 digest: {stated:?}"))?;
    let actual = sha1_smol::Sha1::from(bytes).digest().to_string();
    if actual != expected {
        bail!("{binary_url} failed its checksum: expected sha1 {expected}, got {actual}");
    }
    Ok(actual)
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

    const REPO: &str = "https://repo.test/maven2";

    fn sha1_hex(bytes: &[u8]) -> String {
        sha1_smol::Sha1::from(bytes).digest().to_string()
    }

    fn metadata(versions: &[&str]) -> Vec<u8> {
        let list: String = versions.iter().map(|v| format!("<version>{v}</version>")).collect();
        format!(
            "<?xml version='1.0'?><metadata><versioning><latest>9.9.9-alpha01</latest>\
             <versions>{list}</versions></versioning></metadata>"
        )
        .into_bytes()
    }

    /// Publish `group:artifact:version` in `repo` with the given packaging
    /// and binary; `.sha1` is correct unless `sha1` overrides it.
    fn publish(
        repo: &mut FakeRepo,
        group: &str,
        artifact: &str,
        versions: &[&str],
        version: &str,
        ext: &str,
        binary: &[u8],
    ) {
        let dir = artifact_dir_url(REPO, group, artifact);
        repo.files.insert(format!("{dir}/maven-metadata.xml"), metadata(versions));
        let base = format!("{dir}/{version}/{artifact}-{version}");
        repo.files.insert(
            format!("{base}.pom"),
            format!("<project><packaging>{ext}</packaging></project>").into_bytes(),
        );
        repo.files.insert(format!("{base}.{ext}"), binary.to_vec());
        repo.files.insert(format!("{base}.{ext}.sha1"), sha1_hex(binary).into_bytes());
    }

    #[test]
    fn artifact_dir_url_maps_group_dots_to_path_segments() {
        assert_eq!(
            artifact_dir_url(
                "https://dl.google.com/android/maven2",
                "androidx.customview",
                "customview-poolingcontainer"
            ),
            "https://dl.google.com/android/maven2/androidx/customview/customview-poolingcontainer"
        );
    }

    #[test]
    fn metadata_versions_are_read_from_the_versions_block_only() {
        let xml = "<metadata><versioning><latest>1.1.0</latest><release>1.1.0</release>\
            <versions>\n  <version>1.0.0-alpha01</version>\n  <version>1.0.0</version>\n\
            <version>1.1.0</version>\n</versions></versioning></metadata>";
        assert_eq!(parse_metadata_versions(xml), vec!["1.0.0-alpha01", "1.0.0", "1.1.0"]);
        assert!(parse_metadata_versions("<metadata/>").is_empty());
    }

    #[test]
    fn newest_stable_skips_prereleases_and_orders_numerically() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            newest_stable(&v(&["1.9.0", "1.10.0", "1.11.0-rc01", "2.0.0-alpha01"])).as_deref(),
            Some("1.10.0")
        );
        assert_eq!(newest_stable(&v(&["1.0.0-beta01"])), None);
    }

    #[test]
    fn choose_version_prefers_the_group_version_when_published() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let published = v(&["1.12.0", "1.13.0", "1.19.1"]);
        assert_eq!(choose_version(&published, Some("1.12.0")).as_deref(), Some("1.12.0"));
        assert_eq!(choose_version(&published, Some("1.11.0")).as_deref(), Some("1.19.1"));
        assert_eq!(choose_version(&published, None).as_deref(), Some("1.19.1"));
    }

    /// Regression: with `core` 1.12.0 in the gradle cache, the newest
    /// `core-ktx` (1.19.1, an empty jar since 1.13 moved its classes into
    /// `core`) was downloaded, and the first `RecyclerView` detach crashed
    /// with `NoClassDefFoundError: androidx/core/view/ViewGroupKt`. The
    /// group's version must win.
    #[test]
    fn regression_download_aligns_with_the_cached_group_version() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let versions = ["1.12.0", "1.19.1"];
        publish(&mut repo, "androidx.core", "core-ktx", &versions, "1.12.0", "aar", b"ktx-1.12");
        publish(&mut repo, "androidx.core", "core-ktx", &versions, "1.19.1", "aar", b"empty");
        let p = fetch_from_repos(&repo, &[REPO], cache.path(), "androidx.core", "core-ktx",
            Some("1.12.0")).unwrap();
        assert!(p.ends_with("core-ktx-1.12.0.aar"), "{}", p.display());
        assert_eq!(fs::read(&p).unwrap(), b"ktx-1.12");
    }

    #[test]
    fn pom_packaging_and_sha1_file_parse() {
        assert_eq!(
            pom_packaging("<project><packaging>aar</packaging><dependencies/></project>").as_deref(),
            Some("aar")
        );
        assert_eq!(pom_packaging("<project></project>"), None);
        let d = "95994b41b2e6e8c98b56f8cf96269495cc2855b1";
        assert_eq!(parse_sha1_file(d).as_deref(), Some(d));
        assert_eq!(parse_sha1_file(&format!("{d}  foo.aar\n")).as_deref(), Some(d));
        assert_eq!(parse_sha1_file("<html>not found</html>"), None);
    }

    #[test]
    fn module_relocation_is_detected_from_available_at() {
        let kmp = r#"{"variants":[{"name":"jvmApiElements-published",
            "available-at":{"url":"../../collection-jvm/1.4.0/collection-jvm-1.4.0.module",
            "group":"androidx.collection","module":"collection-jvm","version":"1.4.0"}}]}"#;
        let plain = r#"{"variants":[{"name":"releaseVariantReleaseApiPublication","files":[]}]}"#;
        assert!(module_is_relocated(kmp));
        assert!(!module_is_relocated(plain));
        assert!(!module_is_relocated("not json"));
    }

    #[test]
    fn download_is_verified_and_stored_in_gradle_layout() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        publish(&mut repo, "androidx.customview", "customview-poolingcontainer",
            &["1.0.0", "1.1.0-rc01", "1.1.0"], "1.1.0", "aar", b"aar-bytes");
        let p = fetch_from_repos(&repo, &[REPO], cache.path(), "androidx.customview",
            "customview-poolingcontainer", None).unwrap();
        let expected = cache
            .path()
            .join("androidx.customview/customview-poolingcontainer/1.1.0")
            .join(sha1_hex(b"aar-bytes"))
            .join("customview-poolingcontainer-1.1.0.aar");
        assert_eq!(p, expected);
        assert_eq!(fs::read(&p).unwrap(), b"aar-bytes");
    }

    #[test]
    fn checksum_mismatch_is_refused_and_nothing_is_cached() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        publish(&mut repo, "androidx.core", "core-ktx", &["1.0.0"], "1.0.0", "aar", b"good");
        let url = format!("{}/1.0.0/core-ktx-1.0.0.aar", artifact_dir_url(REPO, "androidx.core", "core-ktx"));
        repo.files.insert(url, b"tampered".to_vec());
        let err = fetch_from_repos(&repo, &[REPO], cache.path(), "androidx.core", "core-ktx", None)
            .unwrap_err();
        assert!(format!("{err:#}").contains("checksum"), "{err:#}");
        assert!(!cache.path().join("androidx.core").exists());
    }

    #[test]
    fn missing_sha1_is_refused() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        publish(&mut repo, "androidx.core", "core-ktx", &["1.0.0"], "1.0.0", "aar", b"good");
        repo.files.retain(|k, _| !k.ends_with(".sha1"));
        let err = fetch_from_repos(&repo, &[REPO], cache.path(), "androidx.core", "core-ktx", None)
            .unwrap_err();
        assert!(format!("{err:#}").contains("unverified"), "{err:#}");
    }

    #[test]
    fn kmp_relocated_base_fetches_the_jvm_variant() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        let dir = artifact_dir_url(REPO, "androidx.collection", "collection");
        repo.files.insert(format!("{dir}/maven-metadata.xml"), metadata(&["1.4.0"]));
        repo.files.insert(format!("{dir}/1.4.0/collection-1.4.0.pom"), b"<project/>".to_vec());
        repo.files.insert(
            format!("{dir}/1.4.0/collection-1.4.0.module"),
            br#"{"variants":[{"available-at":{"module":"collection-jvm"}}]}"#.to_vec(),
        );
        // The base's metadata-only jar must never be fetched.
        repo.files.insert(format!("{dir}/1.4.0/collection-1.4.0.jar"), b"klib".to_vec());
        publish(&mut repo, "androidx.collection", "collection-jvm", &["1.4.0"], "1.4.0", "jar", b"classes");
        let p = fetch_from_repos(&repo, &[REPO], cache.path(), "androidx.collection", "collection", None)
            .unwrap();
        assert!(p.ends_with("collection-jvm-1.4.0.jar"), "{}", p.display());
        assert!(!repo.requested.borrow().iter().any(|u| u.ends_with("collection-1.4.0.jar")));
    }

    #[test]
    fn a_repository_without_the_artifact_falls_through_to_the_next() {
        let cache = tempfile::tempdir().unwrap();
        let mut repo = FakeRepo::default();
        publish(&mut repo, "org.jetbrains.kotlinx", "kotlinx-coroutines-android",
            &["1.8.1"], "1.8.1", "jar", b"jar");
        let p = fetch_from_repos(&repo, &["https://other.test/m2", REPO], cache.path(),
            "org.jetbrains.kotlinx", "kotlinx-coroutines-android", None).unwrap();
        assert!(p.ends_with("kotlinx-coroutines-android-1.8.1.jar"));
    }

    #[test]
    fn unpublished_everywhere_names_every_repository() {
        let cache = tempfile::tempdir().unwrap();
        let err = fetch_from_repos(&FakeRepo::default(), &["https://a.test", "https://b.test"],
            cache.path(), "androidx.nope", "nope", None).unwrap_err().to_string();
        assert!(err.contains("https://a.test") && err.contains("https://b.test"), "{err}");
    }

    /// Hits the real Google Maven repository. Run with `--ignored`.
    #[test]
    #[ignore = "network: downloads from dl.google.com"]
    fn live_google_maven_serves_customview_poolingcontainer() {
        let cache = tempfile::tempdir().unwrap();
        let p = fetch_from_maven(&Curl, cache.path(), "androidx.customview",
            "customview-poolingcontainer", None).unwrap();
        assert_eq!(p.extension().unwrap(), "aar");
        assert!(fs::metadata(&p).unwrap().len() > 0);
    }
}
