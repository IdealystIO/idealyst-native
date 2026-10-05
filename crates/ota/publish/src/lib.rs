//! Publishing over-the-air releases: bundles to `bundles/<name>/<sha>.wasm`,
//! then the index apps read (`ota-index`).
//!
//! The order matters: a bundle file is uploaded before the index names it,
//! so an app never reads an index pointing at a file that isn't there yet.
//! Bundle files never change (their name is their hash), so a CDN may cache
//! them forever; the index is served revalidated.
//!
//! Two people publishing at once can't lose each other's release: the
//! index is written only if it is still the version this publish read
//! (S3 conditional writes), and is re-read and re-applied otherwise.
//!
//! S3 is reached through the `aws` CLI, as the registry tool does: the
//! publisher's own credentials and profile, no SDK in the build.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use ota_index::{new_requirements, Bundle, Index, Release, INDEX_FILE, KEEP};

/// The index is checked on every app launch: never served stale.
const INDEX_CACHE: &str = "public, max-age=0, must-revalidate";
/// A bundle file's name is its hash: it never changes.
const BUNDLE_CACHE: &str = "public, max-age=31536000, immutable";

/// Where releases are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `s3://bucket/prefix`.
    S3 { bucket: String, prefix: String },
    /// A directory (`file://…`, or a plain path): for development, tests,
    /// or a release location synced by other means.
    Dir(PathBuf),
}

impl Target {
    pub fn parse(s: &str) -> Result<Target> {
        if let Some(rest) = s.strip_prefix("s3://") {
            let (bucket, prefix) = rest.split_once('/').unwrap_or((rest, ""));
            if bucket.is_empty() {
                bail!("`{s}`: no bucket");
            }
            return Ok(Target::S3 { bucket: bucket.into(), prefix: prefix.trim_matches('/').into() });
        }
        if s.contains("://") && !s.starts_with("file://") {
            bail!("`{s}`: a release location is `s3://bucket/prefix` or a directory");
        }
        Ok(Target::Dir(PathBuf::from(s.strip_prefix("file://").unwrap_or(s))))
    }
}

/// What a conditional write expects to replace.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Expect {
    /// Nothing: the object must not exist yet.
    Absent,
    /// This version (an S3 ETag; a directory's content hash).
    Version(String),
}

enum Written {
    Done,
    /// Someone else wrote it since it was read.
    Conflict,
}

impl Target {
    fn key(prefix: &str, path: &str) -> String {
        if prefix.is_empty() { path.to_string() } else { format!("{prefix}/{path}") }
    }

    /// `path`'s bytes and version, or `None` if it doesn't exist.
    fn get(&self, path: &str) -> Result<Option<(Vec<u8>, String)>> {
        match self {
            Target::Dir(dir) => match std::fs::read(dir.join(path)) {
                Ok(b) => {
                    let v = remote_bundle::content_hash(&b);
                    Ok(Some((b, v)))
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(e) => Err(e).with_context(|| format!("read {}", dir.join(path).display())),
            },
            Target::S3 { bucket, prefix } => {
                let tmp = tempfile_path("get");
                let out = Command::new("aws")
                    .args(["s3api", "get-object", "--bucket", bucket, "--key", &Self::key(prefix, path)])
                    .arg(&tmp)
                    .output()
                    .context("run `aws s3api get-object` (is the AWS CLI installed?)")?;
                if !out.status.success() {
                    let err = String::from_utf8_lossy(&out.stderr);
                    if err.contains("NoSuchKey") {
                        return Ok(None);
                    }
                    bail!("read s3://{bucket}/{}: {err}", Self::key(prefix, path));
                }
                let meta: serde_json::Value = serde_json::from_slice(&out.stdout).context("read get-object's reply")?;
                let etag = meta["ETag"].as_str().ok_or_else(|| anyhow!("get-object returned no ETag"))?.to_string();
                let bytes = std::fs::read(&tmp)?;
                let _ = std::fs::remove_file(&tmp);
                Ok(Some((bytes, etag)))
            }
        }
    }

    fn put(&self, path: &str, bytes: &[u8], content_type: &str, cache: &str, expect: Option<&Expect>) -> Result<Written> {
        match self {
            Target::Dir(dir) => {
                let file = dir.join(path);
                if let Some(expect) = expect {
                    let current = std::fs::read(&file).ok().map(|b| remote_bundle::content_hash(&b));
                    let fits = match expect {
                        Expect::Absent => current.is_none(),
                        Expect::Version(v) => current.as_deref() == Some(v),
                    };
                    if !fits {
                        return Ok(Written::Conflict);
                    }
                }
                std::fs::create_dir_all(file.parent().expect("a file in the directory"))?;
                // Written beside it, then renamed: an app never reads half.
                let tmp = file.with_extension("partial");
                std::fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
                std::fs::rename(&tmp, &file)?;
                Ok(Written::Done)
            }
            Target::S3 { bucket, prefix } => {
                let tmp = tempfile_path("put");
                std::fs::write(&tmp, bytes)?;
                let mut cmd = Command::new("aws");
                cmd.args(["s3api", "put-object", "--bucket", bucket, "--key", &Self::key(prefix, path)])
                    .arg("--body")
                    .arg(&tmp)
                    .args(["--content-type", content_type, "--cache-control", cache]);
                match expect {
                    Some(Expect::Absent) => {
                        cmd.args(["--if-none-match", "*"]);
                    }
                    Some(Expect::Version(etag)) => {
                        cmd.args(["--if-match", etag]);
                    }
                    None => {}
                }
                let out = cmd.output().context("run `aws s3api put-object` (is the AWS CLI installed?)")?;
                let _ = std::fs::remove_file(&tmp);
                if out.status.success() {
                    return Ok(Written::Done);
                }
                let err = String::from_utf8_lossy(&out.stderr);
                if err.contains("PreconditionFailed") || err.contains("ConditionalRequestConflict") {
                    return Ok(Written::Conflict);
                }
                bail!("write s3://{bucket}/{}: {err}", Self::key(prefix, path))
            }
        }
    }
}

fn tempfile_path(what: &str) -> PathBuf {
    std::env::temp_dir().join(format!("ota-{what}-{}-{}", std::process::id(), now_nanos()))
}

fn now_nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

/// The release index at `target` (empty if nothing was published yet).
pub fn read_index(target: &Target) -> Result<Index> {
    Ok(read(target)?.0)
}

fn read(target: &Target) -> Result<(Index, Option<String>)> {
    match target.get(INDEX_FILE)? {
        None => Ok((Index::new(), None)),
        Some((bytes, version)) => Ok((Index::parse(&bytes).map_err(|e| anyhow!(e))?, Some(version))),
    }
}

/// Rewrite the index with `change`, re-reading and re-applying it if
/// someone else wrote the index meanwhile.
fn update<T>(target: &Target, mut change: impl FnMut(&mut Index) -> Result<T>) -> Result<T> {
    for _ in 0..5 {
        let (mut index, version) = read(target)?;
        let out = change(&mut index)?;
        let expect = match version {
            None => Expect::Absent,
            Some(v) => Expect::Version(v),
        };
        match target.put(INDEX_FILE, &index.to_json(), "application/json", INDEX_CACHE, Some(&expect))? {
            Written::Done => return Ok(out),
            Written::Conflict => continue,
        }
    }
    bail!("the index kept changing while publishing: someone else is publishing; try again")
}

/// A bundle to publish: its name, and its release build
/// (`idealyst build --remote`'s `<name>.wasm`).
pub struct Upload {
    pub name: String,
    pub wasm: Vec<u8>,
}

/// What publishing a bundle does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub bundle: String,
    pub version: String,
    /// The same bytes are already its newest release: nothing to do.
    pub unchanged: bool,
    /// What it requires that its current release didn't
    /// (`ota_index::new_requirements`): apps lacking it keep the current
    /// release. Empty for a bundle's first release.
    pub new_requirements: Vec<String>,
}

/// The release a built bundle makes.
fn release(upload: &Upload, published: u64) -> Result<Release> {
    let meta = remote_bundle::metadata(&upload.wasm)?
        .ok_or_else(|| anyhow!("`{}` isn't a release build (no metadata): build it with `idealyst build --remote`", upload.name))?;
    let requires = remote_bundle::requires(&upload.wasm)?.ok_or_else(|| {
        anyhow!("`{}` records no requirements: rebuild it with this version of `idealyst build --remote`", upload.name)
    })?;
    let signed_by = remote_bundle::signature(&upload.wasm).map_err(|e| anyhow!("`{}`: {e}", upload.name))?.map(|s| s.key.to_string());
    let sha256 = remote_bundle::content_hash(&upload.wasm);
    Ok(Release {
        version: meta.version,
        file: Release::path_for(&upload.name, &sha256),
        sha256,
        size: upload.wasm.len() as u64,
        codec: meta.codec,
        signed_by,
        published,
        requires,
    })
}

/// What publishing `uploads` over `index` would do.
pub fn plan(index: &Index, uploads: &[Upload]) -> Result<Vec<Planned>> {
    uploads
        .iter()
        .map(|u| {
            let next = release(u, 0)?;
            let current = index.bundles.get(&u.name).and_then(|b| b.releases.first());
            Ok(Planned {
                bundle: u.name.clone(),
                version: next.version.clone(),
                unchanged: current.is_some_and(|c| c.sha256 == next.sha256),
                new_requirements: current.map(|c| new_requirements(&c.requires, &next.requires)).unwrap_or_default(),
            })
        })
        .collect()
}

/// Publish `uploads` to `target`: each bundle's file, then the index with
/// each as its bundle's newest release (oldest beyond [`KEEP`] dropped from
/// the index; their files stay). `published` is the time to record,
/// seconds since the Unix epoch.
pub fn publish(target: &Target, uploads: &[Upload], published: u64) -> Result<Vec<Planned>> {
    let releases: Vec<Release> = uploads.iter().map(|u| release(u, published)).collect::<Result<_>>()?;
    for (u, r) in uploads.iter().zip(&releases) {
        // Same name, same bytes: an existing file is already right.
        match target.put(&r.file, &u.wasm, "application/wasm", BUNDLE_CACHE, Some(&Expect::Absent))? {
            Written::Done | Written::Conflict => {}
        }
    }
    update(target, |index| {
        let planned = plan(index, uploads)?;
        for ((u, r), p) in uploads.iter().zip(&releases).zip(&planned) {
            if p.unchanged {
                continue;
            }
            let bundle = index.bundles.entry(u.name.clone()).or_insert_with(Bundle::default);
            bundle.releases.insert(0, r.clone());
            bundle.releases.truncate(KEEP);
        }
        Ok(planned)
    })
}

/// Take back `bundle`'s newest release: apps go back to the one before
/// (each to the newest it can run) at their next check. The release is kept
/// as withdrawn. Returns the release now newest, if any.
pub fn rollback(target: &Target, bundle: &str) -> Result<Option<Release>> {
    update(target, |index| {
        let b = index.bundles.get_mut(bundle).ok_or_else(|| anyhow!("nothing published for bundle `{bundle}`"))?;
        if b.releases.len() < 2 {
            bail!("bundle `{bundle}` has no earlier release to go back to");
        }
        let withdrawn = b.releases.remove(0);
        b.withdrawn.insert(0, withdrawn);
        b.withdrawn.truncate(KEEP);
        Ok(b.releases.first().cloned())
    })
}

/// The directory a [`Target::Dir`] writes to (tests, `file://` URLs).
pub fn dir(target: &Target) -> Option<&Path> {
    match target {
        Target::Dir(d) => Some(d),
        Target::S3 { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_parse() {
        assert_eq!(
            Target::parse("s3://my-ota/app/prod").unwrap(),
            Target::S3 { bucket: "my-ota".into(), prefix: "app/prod".into() }
        );
        assert_eq!(Target::parse("s3://my-ota").unwrap(), Target::S3 { bucket: "my-ota".into(), prefix: "".into() });
        assert_eq!(Target::parse("file:///tmp/x").unwrap(), Target::Dir("/tmp/x".into()));
        assert_eq!(Target::parse("dist/ota").unwrap(), Target::Dir("dist/ota".into()));
        assert!(Target::parse("https://cdn.example.com").is_err(), "apps read URLs; publishing writes a bucket");
    }

    /// The directory store refuses a write that would overwrite an index
    /// someone else changed since it was read.
    #[test]
    fn a_stale_index_write_is_a_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let t = Target::Dir(dir.path().into());
        assert!(matches!(t.put("index.json", b"a", "", "", Some(&Expect::Absent)).unwrap(), Written::Done));
        let (_, v) = t.get("index.json").unwrap().unwrap();
        assert!(matches!(t.put("index.json", b"b", "", "", Some(&Expect::Absent)).unwrap(), Written::Conflict));
        assert!(matches!(t.put("index.json", b"b", "", "", Some(&Expect::Version(v.clone()))).unwrap(), Written::Done));
        assert!(matches!(t.put("index.json", b"c", "", "", Some(&Expect::Version(v))).unwrap(), Written::Conflict));
    }
}
