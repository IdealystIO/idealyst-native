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
//! publisher's own credentials and profile, no SDK in the build. A server
//! (the console) reaches it over HTTP with configured keys instead
//! ([`S3Http`], feature `s3-http`).
//!
//! Besides publishing, a location's releases are managed here: take one
//! down (the kill switch with it), restore it, pin one ahead of newer ones.
//! Every change is recorded in `audit.json` beside the index.
//!
//! App builds are registered here too ([`register`]): each one's manifest
//! under its id, and — for a build captured at build time — its answer
//! (`resolved/<id>.json`), which every change to the index rewrites, so an
//! app can fetch its own answer instead of the whole index.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context, Result};
use ota_index::{
    manifest_path, new_requirements, reported_path, resolve, resolved_path, Bundle, Index, Manifest, ManifestSource, Registered, Registry,
    Release, Resolution, Withdrawn, INDEX_FILE, KEEP, MANIFESTS_FILE, REPORTED_DIR,
};
use serde::{Deserialize, Serialize};

#[cfg(feature = "s3-http")]
mod s3http;
#[cfg(feature = "s3-http")]
pub use s3http::S3Http;

/// The audit log's file name, beside the index. Never read by apps.
pub const AUDIT_FILE: &str = "audit.json";
/// How many events the audit log keeps.
const AUDIT_KEEP: usize = 1000;

/// The index is checked on every app launch: never served stale.
const INDEX_CACHE: &str = "public, max-age=0, must-revalidate";
/// A bundle file's name is its hash: it never changes.
const BUNDLE_CACHE: &str = "public, max-age=31536000, immutable";
/// How many times a publish re-reads and re-applies an index someone else
/// changed before giving up ([`update`]).
const ATTEMPTS: usize = 16;

/// Where releases are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// `s3://bucket/prefix`.
    S3 { bucket: String, prefix: String },
    /// A directory (`file://…`, or a plain path): for development, tests,
    /// or a release location synced by other means.
    Dir(PathBuf),
    /// S3 over HTTP, with configured keys (the console).
    #[cfg(feature = "s3-http")]
    S3Http(S3Http),
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Target::S3 { bucket, prefix } => write!(f, "s3://{bucket}/{prefix}"),
            Target::Dir(d) => write!(f, "{}", d.display()),
            #[cfg(feature = "s3-http")]
            Target::S3Http(s) => write!(f, "s3://{}/{} at {}", s.bucket, s.prefix, s.endpoint),
        }
    }
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

/// The release location `location` names, reached the way a server reaches
/// it: `s3://bucket/prefix` over signed HTTP with the AWS_* variables
/// ([`S3Http::from_env`]), anything else as [`Target::parse`] reads it.
#[cfg(feature = "s3-http")]
pub fn connect(location: &str) -> Result<Target> {
    if location.starts_with("s3://") {
        return S3Http::from_env(location).map(Target::S3Http);
    }
    Target::parse(location)
}

/// `2026-10-06T11:13:37.000Z` / `…+00:00` (UTC, as S3 writes them) as
/// seconds since the Unix epoch.
pub(crate) fn iso_secs(s: &str) -> Option<u64> {
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, m, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hh, mm, ss) = (num(11..13)?, num(14..16)?, num(17..19)?);
    // Howard Hinnant's days-from-civil.
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    u64::try_from(days * 86_400 + hh * 3_600 + mm * 60 + ss).ok()
}

/// What a conditional write expects to replace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Expect {
    /// Nothing: the object must not exist yet.
    Absent,
    /// This version (an S3 ETag; a directory's content hash).
    Version(String),
}

pub(crate) enum Written {
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
            #[cfg(feature = "s3-http")]
            Target::S3Http(s3) => s3.get(path),
        }
    }

    /// Every object under `prefix`: its path from the location's root, and
    /// when it was last written (seconds since the Unix epoch).
    fn list(&self, prefix: &str) -> Result<Vec<(String, u64)>> {
        match self {
            Target::Dir(dir) => {
                let base = dir.join(prefix);
                let entries = match std::fs::read_dir(&base) {
                    Ok(e) => e,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
                    Err(e) => return Err(e).with_context(|| format!("list {}", base.display())),
                };
                let mut out = Vec::new();
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if name.ends_with(".partial") {
                        continue;
                    }
                    let modified = entry
                        .metadata()
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_secs());
                    out.push((format!("{prefix}{name}"), modified));
                }
                Ok(out)
            }
            Target::S3 { bucket, prefix: root } => {
                let out = Command::new("aws")
                    .args(["s3api", "list-objects-v2", "--bucket", bucket, "--prefix", &Self::key(root, prefix), "--output", "json"])
                    .output()
                    .context("run `aws s3api list-objects-v2` (is the AWS CLI installed?)")?;
                if !out.status.success() {
                    bail!("list s3://{bucket}/{}: {}", Self::key(root, prefix), String::from_utf8_lossy(&out.stderr));
                }
                if out.stdout.iter().all(u8::is_ascii_whitespace) {
                    return Ok(Vec::new());
                }
                let reply: serde_json::Value = serde_json::from_slice(&out.stdout).context("read list-objects-v2's reply")?;
                let strip = if root.is_empty() { String::new() } else { format!("{root}/") };
                Ok(reply["Contents"]
                    .as_array()
                    .map(|objects| {
                        objects
                            .iter()
                            .filter_map(|o| {
                                let key = o["Key"].as_str()?;
                                let modified = o["LastModified"].as_str().and_then(iso_secs).unwrap_or(0);
                                Some((key.strip_prefix(&strip).unwrap_or(key).to_string(), modified))
                            })
                            .collect()
                    })
                    .unwrap_or_default())
            }
            #[cfg(feature = "s3-http")]
            Target::S3Http(s3) => s3.list(prefix),
        }
    }

    fn put(&self, path: &str, bytes: &[u8], content_type: &str, cache: &str, expect: Option<&Expect>) -> Result<Written> {
        match self {
            // Check-then-rename: not atomic against another process
            // publishing to the same directory at the same instant. A
            // directory is for development and tests; S3's conditional
            // writes are.
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
            #[cfg(feature = "s3-http")]
            Target::S3Http(s3) => s3.put(path, bytes, content_type, cache, expect),
        }
    }
}

/// A scratch file for one `aws` call. A counter makes it unique within the
/// process: the clock alone isn't — macOS's has microsecond resolution, and
/// racing publishes in one process collided on it (one deleted the other's
/// download mid-read).
fn tempfile_path(what: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("ota-{what}-{}-{n}", std::process::id()))
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
///
/// Each conflict means another publish's write landed, so with `n`
/// publishing at once the last one may lose `n - 1` times: the limit is
/// generous, and a short jittered pause keeps them from colliding in step.
///
/// Every write bumps the index's `generation`, which the answers made from
/// it record ([`resolve_registered`]).
///
/// An index read at generation 0 was last written by something that
/// doesn't record one: never written yet, or rewritten by an older CLI
/// (`ota-publish` 0.1), which drops the field. Counting up from 0 again
/// would give answers [`write_answer`] refuses, since the stored ones
/// record the generation from before — every precomputed answer would
/// stay stale. So the count resumes past the newest stored answer
/// ([`answers_generation`]). A listing, paid only on such a write.
fn update<T>(target: &Target, mut change: impl FnMut(&mut Index) -> Result<T>) -> Result<T> {
    update_file(target, INDEX_FILE, Index::new, |b| Index::parse(b).map_err(|e| anyhow!(e)), Index::to_json, |index| {
        let out = change(index)?;
        if index.generation == 0 {
            index.generation = answers_generation(target)?;
        }
        index.generation += 1;
        Ok(out)
    })
}

/// Where the precomputed answers are (`ota_index::resolved_path`).
const RESOLVED_DIR: &str = "resolved/";

/// The newest generation a stored answer records (0 if none).
fn answers_generation(target: &Target) -> Result<u64> {
    let mut newest = 0;
    for (path, _) in target.list(RESOLVED_DIR)? {
        // One that doesn't parse (a newer format) isn't one this can
        // outrank; `write_answer` leaves it alone anyway.
        if let Some((bytes, _)) = target.get(&path)? {
            if let Ok(answer) = Resolution::parse(&bytes) {
                newest = newest.max(answer.generation);
            }
        }
    }
    Ok(newest)
}

/// The index to make answers from. One at generation 0 while answers
/// record a later one (an older CLI rewrote it, see [`update`]) is first
/// rewritten unchanged, which moves its generation past theirs — else
/// the answers made from it would be refused as older.
fn index_for_answers(target: &Target) -> Result<Index> {
    let index = read_index(target)?;
    if index.generation == 0 && answers_generation(target)? > 0 {
        update(target, |_| Ok(()))?;
        return read_index(target);
    }
    Ok(index)
}

/// [`update`] for any JSON file at the location.
fn update_file<D, T>(
    target: &Target,
    file: &str,
    empty: impl Fn() -> D,
    parse: impl Fn(&[u8]) -> Result<D>,
    write: impl Fn(&D) -> Vec<u8>,
    mut change: impl FnMut(&mut D) -> Result<T>,
) -> Result<T> {
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            let jitter = (now_nanos() % 100) as u64;
            std::thread::sleep(std::time::Duration::from_millis(20 + jitter));
        }
        let (mut doc, version) = match target.get(file)? {
            None => (empty(), None),
            Some((bytes, version)) => (parse(&bytes)?, Some(version)),
        };
        let out = change(&mut doc)?;
        let expect = match version {
            None => Expect::Absent,
            Some(v) => Expect::Version(v),
        };
        match target.put(file, &write(&doc), "application/json", INDEX_CACHE, Some(&expect))? {
            Written::Done => return Ok(out),
            Written::Conflict => continue,
        }
    }
    bail!("`{file}` kept changing while this wrote it: someone else is publishing or managing releases; try again")
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

/// The release a built bundle makes, published at `published`.
pub fn release_of(upload: &Upload, published: u64) -> Result<Release> {
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
            let next = release_of(u, 0)?;
            let bundle = index.bundles.get(&u.name);
            let current = bundle.and_then(|b| b.releases.first());
            Ok(Planned {
                bundle: u.name.clone(),
                version: next.version.clone(),
                // Already a release apps choose from (live, or behind a pin).
                unchanged: bundle.is_some_and(|b| b.releases.iter().any(|r| r.sha256 == next.sha256)),
                new_requirements: current.map(|c| new_requirements(&c.requires, &next.requires)).unwrap_or_default(),
            })
        })
        .collect()
}

/// Publish `uploads` to `target`: each bundle's file, then the index with
/// each as its bundle's newest release — behind a pinned one, if any
/// (oldest beyond [`KEEP`] dropped from the index; their files stay).
/// `published` is the time to record, seconds since the Unix epoch;
/// `actor` who did it, for the audit log.
pub fn publish(target: &Target, uploads: &[Upload], published: u64, actor: &str) -> Result<Vec<Planned>> {
    let releases: Vec<Release> = uploads.iter().map(|u| release_of(u, published)).collect::<Result<_>>()?;
    for (u, r) in uploads.iter().zip(&releases) {
        // Same name, same bytes: an existing file is already right.
        match target.put(&r.file, &u.wasm, "application/wasm", BUNDLE_CACHE, Some(&Expect::Absent))? {
            Written::Done | Written::Conflict => {}
        }
    }
    let planned = update(target, |index| {
        let planned = plan(index, uploads)?;
        for ((u, r), p) in uploads.iter().zip(&releases).zip(&planned) {
            if p.unchanged {
                continue;
            }
            let bundle = index.bundles.entry(u.name.clone()).or_insert_with(Bundle::default);
            bundle.releases.push(r.clone());
            bundle.normalize();
            bundle.releases.truncate(KEEP);
        }
        Ok(planned)
    })?;
    for (p, r) in planned.iter().zip(&releases) {
        if !p.unchanged {
            record(target, Event::new(actor, Action::Publish, &p.bundle, r, None))?;
        }
    }
    after_change(target)?;
    Ok(planned)
}

/// Take back `bundle`'s live release (`idealyst ota rollback`): apps return
/// to the one before at their next check. Returns the release now live.
pub fn rollback(target: &Target, bundle: &str, actor: &str) -> Result<Option<Release>> {
    let index = read_index(target)?;
    let releases = &index.bundles.get(bundle).ok_or_else(|| anyhow!("nothing published for bundle `{bundle}`"))?.releases;
    if releases.len() < 2 {
        bail!("bundle `{bundle}` has no earlier release to go back to");
    }
    take_down(target, bundle, &releases[0].sha256, TakeDown::default(), actor)
}

/// How a release is taken down.
#[derive(Debug, Clone, Default)]
pub struct TakeDown {
    /// The kill switch: apps running it replace it at once.
    pub urgent: bool,
    pub reason: Option<String>,
}

/// Take release `sha256` of `bundle` down: apps stop choosing it and go to
/// the newest remaining release they can run — at once if `urgent`, else
/// when they'd apply any update. Taking down the last release leaves apps
/// without one: urgent, they drop it (or return to their built-in copy);
/// otherwise they keep what they run. Returns the release now live.
pub fn take_down(target: &Target, bundle: &str, sha256: &str, how: TakeDown, actor: &str) -> Result<Option<Release>> {
    let mut taken = None;
    let live = update(target, |index| {
        let b = index.bundles.get_mut(bundle).ok_or_else(|| anyhow!("nothing published for bundle `{bundle}`"))?;
        let i = b.releases.iter().position(|r| r.sha256 == sha256).ok_or_else(|| anyhow!("`{bundle}` has no live release {sha256}"))?;
        let release = b.releases.remove(i);
        if b.pinned.as_deref() == Some(sha256) {
            b.pinned = None;
        }
        taken = Some(release.clone());
        b.withdrawn.insert(0, Withdrawn { release, withdrawn_at: now_secs(), urgent: how.urgent, reason: how.reason.clone() });
        b.withdrawn.truncate(KEEP);
        b.normalize();
        Ok(b.releases.first().cloned())
    })?;
    let taken = taken.expect("set when the update succeeds");
    let action = if how.urgent { Action::KillSwitch } else { Action::TakeDown };
    record(target, Event::new(actor, action, bundle, &taken, how.reason))?;
    after_change(target)?;
    Ok(live)
}

/// Put a taken-down release back where apps choose from, at its place by
/// publish time. Returns the release now live.
pub fn restore(target: &Target, bundle: &str, sha256: &str, actor: &str) -> Result<Option<Release>> {
    let mut restored = None;
    let live = update(target, |index| {
        let b = index.bundles.get_mut(bundle).ok_or_else(|| anyhow!("nothing published for bundle `{bundle}`"))?;
        let i = b.withdrawn.iter().position(|w| w.release.sha256 == sha256).ok_or_else(|| anyhow!("`{bundle}` has no taken-down release {sha256}"))?;
        let release = b.withdrawn.remove(i).release;
        restored = Some(release.clone());
        b.releases.push(release);
        b.normalize();
        b.releases.truncate(KEEP);
        Ok(b.releases.first().cloned())
    })?;
    record(target, Event::new(actor, Action::Restore, bundle, &restored.expect("set on success"), None))?;
    after_change(target)?;
    Ok(live)
}

/// Serve release `sha256` of `bundle` ahead of newer ones, including ones
/// published later, until [`unpin`]ned.
pub fn pin(target: &Target, bundle: &str, sha256: &str, actor: &str) -> Result<()> {
    let mut pinned = None;
    update(target, |index| {
        let b = index.bundles.get_mut(bundle).ok_or_else(|| anyhow!("nothing published for bundle `{bundle}`"))?;
        let r = b.releases.iter().find(|r| r.sha256 == sha256).ok_or_else(|| anyhow!("`{bundle}` has no live release {sha256}"))?;
        pinned = Some(r.clone());
        b.pinned = Some(sha256.to_string());
        b.normalize();
        Ok(())
    })?;
    record(target, Event::new(actor, Action::Pin, bundle, &pinned.expect("set on success"), None))?;
    after_change(target)
}

/// Serve `bundle`'s newest release again.
pub fn unpin(target: &Target, bundle: &str, actor: &str) -> Result<()> {
    let mut was = None;
    update(target, |index| {
        let b = index.bundles.get_mut(bundle).ok_or_else(|| anyhow!("nothing published for bundle `{bundle}`"))?;
        was = b.pinned.take().and_then(|sha| b.releases.iter().find(|r| r.sha256 == sha).cloned());
        b.normalize();
        Ok(())
    })?;
    if let Some(r) = was {
        record(target, Event::new(actor, Action::Unpin, bundle, &r, None))?;
    }
    after_change(target)
}

/// The index changed: rewrite the registered builds' answers. The change
/// itself is already live, so a failure here says that, and how to finish.
fn after_change(target: &Target) -> Result<()> {
    let r = resolve_registered(target)?;
    if r.failed.is_empty() {
        return Ok(());
    }
    let list: Vec<String> = r.failed.iter().map(|(id, e)| format!("{}: {e}", short(id))).collect();
    bail!(
        "the change is live, but {} registered app build(s) still have their previous answer ({}). Apps of those builds read the index instead only once that answer is gone; run `idealyst ota resolve` to retry",
        r.failed.len(),
        list.join("; ")
    )
}

fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

/// How long a registered build's answer may be cached: revalidated on
/// every read, like the index it is made from.
const ANSWER_CACHE: &str = INDEX_CACHE;

/// The registered app builds (empty if none): the ones registered from
/// their build (`manifests.json`), then the ones reported from the field
/// (`reported/`), oldest first.
pub fn read_registry(target: &Target) -> Result<Registry> {
    let mut registry = match target.get(MANIFESTS_FILE)? {
        None => Registry::default(),
        Some((bytes, _)) => Registry::parse(&bytes).map_err(|e| anyhow!(e))?,
    };
    let mut reported = reported_ids(target)?;
    reported.sort_by_key(|(id, at)| (*at, id.clone()));
    for (id, at) in reported {
        if registry.get(&id).is_none() {
            registry.manifests.push(Registered { id, source: ManifestSource::Reported, label: None, registered_at: at });
        }
    }
    Ok(registry)
}

/// The ids under `reported/`, with when each was reported.
fn reported_ids(target: &Target) -> Result<Vec<(String, u64)>> {
    Ok(target
        .list(REPORTED_DIR)?
        .into_iter()
        .filter_map(|(path, at)| Some((path.strip_prefix(REPORTED_DIR)?.strip_suffix(".json")?.to_string(), at)))
        .collect())
}

/// The manifest stored under `id`, if any.
pub fn read_manifest(target: &Target, id: &str) -> Result<Option<Manifest>> {
    match target.get(&manifest_path(id))? {
        None => Ok(None),
        Some((bytes, _)) => Manifest::parse(&bytes).map(Some).map_err(|e| anyhow!("{}: {e}", manifest_path(id))),
    }
}

/// The stored answer for manifest `id`, if any.
pub fn read_answer(target: &Target, id: &str) -> Result<Option<Resolution>> {
    match target.get(&resolved_path(id))? {
        None => Ok(None),
        Some((bytes, _)) => Resolution::parse(&bytes).map(Some).map_err(|e| anyhow!(e)),
    }
}

/// What [`register`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registration {
    /// A build the location didn't know.
    Added,
    /// Already registered; nothing changed.
    Known,
    /// Known from the field, now captured from its build (or relabelled).
    Updated,
}

/// Register an app build: its manifest under its id, and how it became
/// known.
///
/// - [`ManifestSource::Build`] (`idealyst ota manifest`): an entry in
///   `manifests.json`, and its answer from the current index, rewritten
///   from then on with every change.
/// - [`ManifestSource::Reported`] (sent by an installed app to a
///   resolution service): a marker, `reported/<id>.json`, and no answer —
///   the service answers it. Nothing shared is rewritten: many instances
///   of a service (a Lambda's, say) reporting a new app release at once
///   each write files of their own, so none waits on, or loses to,
///   another. `max_reported` caps how many are kept, since anyone can send
///   one; past it they are answered but not stored (`Err`). The count is
///   read before writing, so instances racing past the cap can overshoot
///   it by their number.
pub fn register(
    target: &Target,
    manifest: &Manifest,
    source: ManifestSource,
    label: Option<String>,
    max_reported: Option<usize>,
) -> Result<Registration> {
    manifest.verify().map_err(|e| anyhow!(e))?;
    if source == ManifestSource::Reported {
        let reported = reported_ids(target)?;
        if reported.iter().any(|(id, _)| id == &manifest.id) {
            return Ok(Registration::Known);
        }
        if read_registry_file(target)?.get(&manifest.id).is_some() {
            return Ok(Registration::Known);
        }
        if max_reported.is_some_and(|max| reported.len() >= max) {
            bail!("{} reported app builds are already registered, the most this location keeps", reported.len());
        }
    }
    // Its name is its content's hash: one already there is this one.
    match target.put(&manifest_path(&manifest.id), &manifest.to_json(), "application/json", BUNDLE_CACHE, Some(&Expect::Absent))? {
        Written::Done | Written::Conflict => {}
    }
    if source == ManifestSource::Reported {
        let marker = serde_json::json!({ "id": manifest.id, "reported_at": now_secs() });
        let marker = serde_json::to_vec(&marker).expect("a marker serializes");
        return match target.put(&reported_path(&manifest.id), &marker, "application/json", BUNDLE_CACHE, Some(&Expect::Absent))? {
            Written::Done => Ok(Registration::Added),
            Written::Conflict => Ok(Registration::Known),
        };
    }
    let was_reported = target.get(&reported_path(&manifest.id))?.is_some();
    let outcome = update_file(
        target,
        MANIFESTS_FILE,
        Registry::default,
        |b| Registry::parse(b).map_err(|e| anyhow!(e)),
        Registry::to_json,
        |registry| {
            if let Some(entry) = registry.manifests.iter_mut().find(|m| m.id == manifest.id) {
                // An entry from before reports were markers.
                let upgrade = entry.source == ManifestSource::Reported;
                let relabel = label.is_some() && entry.label != label;
                if upgrade || relabel {
                    entry.source = ManifestSource::Build;
                    entry.label = label.clone().or(entry.label.take());
                    return Ok(Registration::Updated);
                }
                return Ok(Registration::Known);
            }
            registry.manifests.push(Registered { id: manifest.id.clone(), source, label: label.clone(), registered_at: now_secs() });
            Ok(if was_reported { Registration::Updated } else { Registration::Added })
        },
    )?;
    write_answer(target, &resolve(&index_for_answers(target)?, &manifest.provides))?;
    Ok(outcome)
}

/// What [`resolve_registered`] did.
#[derive(Debug, Default)]
pub struct Resolved {
    /// Answers written.
    pub written: usize,
    /// Answers already as new (an equal or later index wrote them).
    pub current: usize,
    /// Builds whose answer couldn't be written, and why.
    pub failed: Vec<(String, String)>,
}

/// `manifests.json` alone: the builds registered from their build.
fn read_registry_file(target: &Target) -> Result<Registry> {
    match target.get(MANIFESTS_FILE)? {
        None => Ok(Registry::default()),
        Some((bytes, _)) => Registry::parse(&bytes).map_err(|e| anyhow!(e)),
    }
}

/// Rewrite every build-registered app's answer from the current index
/// (`idealyst ota resolve`; also after every change made here).
pub fn resolve_registered(target: &Target) -> Result<Resolved> {
    let registry = read_registry_file(target)?;
    let mut out = Resolved::default();
    if !registry.manifests.iter().any(|m| m.source == ManifestSource::Build) {
        return Ok(out);
    }
    let index = index_for_answers(target)?;
    for entry in registry.manifests.iter().filter(|m| m.source == ManifestSource::Build) {
        let result = read_manifest(target, &entry.id)
            .and_then(|m| m.ok_or_else(|| anyhow!("its manifest is missing")))
            .and_then(|m| write_answer(target, &resolve(&index, &m.provides)));
        match result {
            Ok(true) => out.written += 1,
            Ok(false) => out.current += 1,
            Err(e) => out.failed.push((entry.id.clone(), format!("{e:#}"))),
        }
    }
    Ok(out)
}

/// Write `answer` unless the stored one was made from an equal or later
/// index: answers written out of order (two changes at once) never move
/// an app back. `Ok(false)` when the stored one is already as new.
fn write_answer(target: &Target, answer: &Resolution) -> Result<bool> {
    let path = resolved_path(&answer.manifest);
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(20 + (now_nanos() % 100) as u64));
        }
        let expect = match target.get(&path)? {
            None => Expect::Absent,
            Some((bytes, version)) => {
                // One that doesn't parse (a newer format) is replaced only
                // by a later index's answer, which this can't tell: leave it.
                match Resolution::parse(&bytes) {
                    Ok(stored) if stored.generation >= answer.generation && stored.rule == answer.rule => return Ok(false),
                    Ok(_) => Expect::Version(version),
                    Err(_) => return Ok(false),
                }
            }
        };
        match target.put(&path, &answer.to_json(), "application/json", ANSWER_CACHE, Some(&expect))? {
            Written::Done => return Ok(true),
            Written::Conflict => continue,
        }
    }
    bail!("`{path}` kept changing while this wrote it")
}

/// What was done.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Action {
    Publish,
    TakeDown,
    KillSwitch,
    Restore,
    Pin,
    Unpin,
}

/// One entry of the audit log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Event {
    /// Seconds since the Unix epoch.
    pub at: u64,
    /// Who: `cli:<user>`, `console`, …
    pub actor: String,
    pub action: Action,
    pub bundle: String,
    pub version: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Event {
    fn new(actor: &str, action: Action, bundle: &str, release: &Release, reason: Option<String>) -> Event {
        Event {
            at: now_secs(),
            actor: actor.to_string(),
            action,
            bundle: bundle.to_string(),
            version: release.version.clone(),
            sha256: release.sha256.clone(),
            reason,
        }
    }
}

/// The audit log, most recent first.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Audit {
    #[serde(default)]
    events: Vec<Event>,
}

/// Append `event` to the audit log. Written after the change it records:
/// the index is what apps follow, so a change that happened is never left
/// unwritten for want of its record.
fn record(target: &Target, event: Event) -> Result<()> {
    update_file(
        target,
        AUDIT_FILE,
        Audit::default,
        |b| serde_json::from_slice(b).context("read the audit log"),
        |a| serde_json::to_vec_pretty(a).expect("the audit log serializes"),
        |audit: &mut Audit| {
            audit.events.insert(0, event.clone());
            audit.events.truncate(AUDIT_KEEP);
            Ok(())
        },
    )
}

/// The audit log, most recent first.
pub fn read_audit(target: &Target) -> Result<Vec<Event>> {
    match target.get(AUDIT_FILE)? {
        None => Ok(Vec::new()),
        Some((bytes, _)) => Ok(serde_json::from_slice::<Audit>(&bytes).context("read the audit log")?.events),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The directory a [`Target::Dir`] writes to (tests, `file://` URLs).
pub fn dir(target: &Target) -> Option<&Path> {
    match target {
        Target::Dir(d) => Some(d),
        _ => None,
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

    /// A release build as far as publishing reads one, made distinct by `n`.
    fn bundle(n: u32) -> Upload {
        let meta = remote_bundle::Metadata { name: "shop".into(), package: "p".into(), version: format!("1.0.{n}"), codec: 2 };
        let wasm = remote_bundle::with_metadata(b"\0asm\x01\0\0\0", &meta).unwrap();
        Upload { name: "shop".into(), wasm: remote_bundle::with_requires(&wasm, &remote_bundle::Requires::default()).unwrap() }
    }

    fn sha(n: u32) -> String {
        remote_bundle::content_hash(&bundle(n).wasm)
    }

    fn live(t: &Target) -> Vec<String> {
        read_index(t).unwrap().bundles["shop"].releases.iter().map(|r| r.version.clone()).collect()
    }

    /// Take down any release (not just the newest), restore it to its place
    /// by publish time, pin one ahead of newer ones (also ones published
    /// later), unpin — each recorded in the audit log, most recent first.
    #[test]
    fn managing_releases() {
        let dir = tempfile::tempdir().unwrap();
        let t = Target::Dir(dir.path().into());
        for n in 1..=3 {
            publish(&t, &[bundle(n)], u64::from(n), "cli:me").unwrap();
        }
        assert_eq!(live(&t), ["1.0.3", "1.0.2", "1.0.1"]);

        let now_live = take_down(&t, "shop", &sha(2), TakeDown { urgent: true, reason: Some("crashes".into()) }, "console").unwrap();
        assert_eq!(now_live.unwrap().version, "1.0.3", "a middle release goes; the live one stays");
        assert_eq!(live(&t), ["1.0.3", "1.0.1"]);
        let shop = read_index(&t).unwrap().bundles["shop"].clone();
        assert!(shop.killed(&sha(2)).is_some_and(|w| w.reason.as_deref() == Some("crashes")));

        restore(&t, "shop", &sha(2), "console").unwrap();
        assert_eq!(live(&t), ["1.0.3", "1.0.2", "1.0.1"], "back in its place");
        assert!(read_index(&t).unwrap().bundles["shop"].withdrawn.is_empty());

        pin(&t, "shop", &sha(1), "console").unwrap();
        assert_eq!(live(&t), ["1.0.1", "1.0.3", "1.0.2"]);
        publish(&t, &[bundle(4)], 4, "cli:me").unwrap();
        assert_eq!(live(&t)[0], "1.0.1", "a newer publish stays behind the pin");
        // Re-publishing the pinned bytes changes nothing.
        assert!(publish(&t, &[bundle(1)], 5, "cli:me").unwrap()[0].unchanged);
        unpin(&t, "shop", "console").unwrap();
        assert_eq!(live(&t), ["1.0.4", "1.0.3", "1.0.2", "1.0.1"]);

        // Taking the pinned release down unpins it.
        pin(&t, "shop", &sha(3), "console").unwrap();
        take_down(&t, "shop", &sha(3), TakeDown::default(), "console").unwrap();
        let shop = read_index(&t).unwrap().bundles["shop"].clone();
        assert_eq!((shop.pinned, shop.releases[0].version.as_str()), (None, "1.0.4"));

        let actions: Vec<(Action, String)> = read_audit(&t).unwrap().iter().map(|e| (e.action, e.version.clone())).collect();
        assert_eq!(
            actions,
            [
                (Action::TakeDown, "1.0.3".into()),
                (Action::Pin, "1.0.3".into()),
                (Action::Unpin, "1.0.1".into()),
                (Action::Publish, "1.0.4".into()),
                (Action::Pin, "1.0.1".into()),
                (Action::Restore, "1.0.2".into()),
                (Action::KillSwitch, "1.0.2".into()),
                (Action::Publish, "1.0.3".into()),
                (Action::Publish, "1.0.2".into()),
                (Action::Publish, "1.0.1".into()),
            ]
        );
    }

    /// An app manifest whose only difference is whether it has the prop the
    /// next release needs.
    fn manifest(with_prop: bool) -> Manifest {
        let mut p = remote_bundle::Provides { codec: 2, ..Default::default() };
        let props = if with_prop { vec![("elevation".to_string(), "u8".to_string())] } else { vec![] };
        p.components.insert("ui::Card".into(), props.into_iter().collect());
        Manifest::new(p)
    }

    /// `bundle(n)`, requiring a prop the old app build lacks.
    fn needs_prop(n: u32) -> Upload {
        let meta = remote_bundle::Metadata { name: "shop".into(), package: "p".into(), version: format!("1.0.{n}"), codec: 2 };
        let wasm = remote_bundle::with_metadata(b"\0asm\x01\0\0\0", &meta).unwrap();
        let mut r = remote_bundle::Requires::default();
        r.components.insert("ui::Card".into(), [("elevation".to_string(), "u8".to_string())].into());
        Upload { name: "shop".into(), wasm: remote_bundle::with_requires(&wasm, &r).unwrap() }
    }

    fn answer(t: &Target, m: &Manifest) -> Option<Resolution> {
        t.get(&resolved_path(&m.id)).unwrap().map(|(b, _)| Resolution::parse(&b).unwrap())
    }

    /// A build registered from its build gets its answer at once, and every
    /// change to the index rewrites it: publish, the kill switch, restore.
    /// A build reported from the field is stored but gets none.
    #[test]
    fn registered_builds_get_answers_that_follow_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let t = Target::Dir(dir.path().into());
        publish(&t, &[bundle(1)], 1, "cli:me").unwrap();
        let (old, new) = (manifest(false), manifest(true));
        assert_eq!(register(&t, &old, ManifestSource::Build, Some("app 1.0".into()), None).unwrap(), Registration::Added);
        assert_eq!(register(&t, &new, ManifestSource::Reported, None, Some(10)).unwrap(), Registration::Added);
        assert_eq!(answer(&t, &old).unwrap().bundles["shop"].release.as_ref().unwrap().version, "1.0.1");
        assert!(answer(&t, &new).is_none(), "a reported build asks the service");

        publish(&t, &[needs_prop(2)], 2, "cli:me").unwrap();
        let a = answer(&t, &old).unwrap();
        assert_eq!(a.generation, read_index(&t).unwrap().generation);
        assert_eq!(a.bundles["shop"].release.as_ref().unwrap().version, "1.0.1", "it can't run 1.0.2");
        assert!(a.bundles["shop"].needs_app_update);

        take_down(&t, "shop", &sha(1), TakeDown { urgent: true, reason: None }, "console").unwrap();
        let a = answer(&t, &old).unwrap();
        assert_eq!((a.bundles["shop"].release.clone(), a.bundles["shop"].killed.clone()), (None, vec![sha(1)]));
        restore(&t, "shop", &sha(1), "console").unwrap();
        assert_eq!(answer(&t, &old).unwrap().bundles["shop"].release.as_ref().unwrap().version, "1.0.1");

        // Captured from its build later: upgraded, and answered from now on.
        assert_eq!(register(&t, &new, ManifestSource::Build, Some("app 1.1".into()), None).unwrap(), Registration::Updated);
        assert_eq!(answer(&t, &new).unwrap().bundles["shop"].release.as_ref().unwrap().version, "1.0.2");
        assert_eq!(register(&t, &new, ManifestSource::Build, Some("app 1.1".into()), None).unwrap(), Registration::Known);
        let registry = read_registry(&t).unwrap();
        let entries: Vec<(ManifestSource, Option<&str>)> = registry.manifests.iter().map(|m| (m.source, m.label.as_deref())).collect();
        assert_eq!(entries, [(ManifestSource::Build, Some("app 1.0")), (ManifestSource::Build, Some("app 1.1"))]);
        assert_eq!(read_manifest(&t, &new.id).unwrap().unwrap(), new);
    }

    /// Anyone can report a manifest: a forged id is refused, and past the
    /// cap reports are no longer stored.
    #[test]
    fn reported_manifests_are_checked_and_capped() {
        let dir = tempfile::tempdir().unwrap();
        let t = Target::Dir(dir.path().into());
        let forged = Manifest { id: manifest(true).id, ..manifest(false) };
        assert!(register(&t, &forged, ManifestSource::Reported, None, None).is_err());
        assert!(t.get(&manifest_path(&forged.id)).unwrap().is_none(), "nothing stored under the forged id");
        register(&t, &manifest(false), ManifestSource::Reported, None, Some(1)).unwrap();
        let err = register(&t, &manifest(true), ManifestSource::Reported, None, Some(1)).unwrap_err();
        assert!(err.to_string().contains("the most this location keeps"), "{err}");
        // A build capture is never capped.
        register(&t, &manifest(true), ManifestSource::Build, None, Some(1)).unwrap();
    }

    /// Regression: reports were appended to `manifests.json` with a
    /// conditional write, so a burst of them — a new app release reaching
    /// many installs at once, each asking a scaled-out resolution service —
    /// all contended for one file, retried, and lost entries or gave up.
    /// Each report now writes files of its own: every one lands.
    #[test]
    fn regression_a_burst_of_reports_loses_none() {
        let dir = tempfile::tempdir().unwrap();
        let t = Target::Dir(dir.path().into());
        let manifests: Vec<Manifest> = (0..32u32)
            .map(|n| {
                let mut p = remote_bundle::Provides { codec: n, ..Default::default() };
                p.components.insert("ui::Card".into(), Default::default());
                Manifest::new(p)
            })
            .collect();
        let threads: Vec<_> = manifests
            .iter()
            .cloned()
            .map(|m| {
                let t = t.clone();
                std::thread::spawn(move || register(&t, &m, ManifestSource::Reported, None, None).unwrap())
            })
            .collect();
        for th in threads {
            assert_eq!(th.join().unwrap(), Registration::Added);
        }
        let registry = read_registry(&t).unwrap();
        assert_eq!(registry.manifests.len(), 32);
        assert!(registry.manifests.iter().all(|m| m.source == ManifestSource::Reported));
        assert!(t.get(MANIFESTS_FILE).unwrap().is_none(), "the shared list isn't touched by reports");
        // Reported again: known, not duplicated.
        assert_eq!(register(&t, &manifests[0], ManifestSource::Reported, None, None).unwrap(), Registration::Known);
    }

    /// Two changes at once can finish their answer writes in either order:
    /// an answer from an older index never replaces one from a newer.
    #[test]
    fn an_older_answer_never_replaces_a_newer_one() {
        let dir = tempfile::tempdir().unwrap();
        let t = Target::Dir(dir.path().into());
        publish(&t, &[bundle(1)], 1, "cli:me").unwrap();
        let m = manifest(false);
        register(&t, &m, ManifestSource::Build, None, None).unwrap();
        let before = read_index(&t).unwrap();
        publish(&t, &[bundle(2)], 2, "cli:me").unwrap();
        let newest = answer(&t, &m).unwrap();
        assert_eq!(newest.bundles["shop"].release.as_ref().unwrap().version, "1.0.2");
        assert!(!write_answer(&t, &resolve(&before, &m.provides)).unwrap(), "the late, older answer is dropped");
        assert_eq!(answer(&t, &m).unwrap(), newest);
    }

    /// Regression: scratch names came from the process id and the clock, and
    /// racing publishes in one process got the same name (macOS's clock has
    /// microsecond resolution): one call deleted another's download.
    #[test]
    fn regression_scratch_files_are_unique_within_a_process() {
        let names: std::collections::HashSet<PathBuf> = (0..1000).map(|_| tempfile_path("get")).collect();
        assert_eq!(names.len(), 1000);
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

    /// What `ota-publish` 0.1 (an older CLI) does to the index when it
    /// publishes `upload`: reads it as its own types and writes them back,
    /// which drops `generation`, `pinned` and what a take-down recorded,
    /// and rewrites no answer.
    fn publish_with_0_1(t: &Target, upload: Upload, published: u64) {
        let (bytes, _) = t.get(INDEX_FILE).unwrap().unwrap();
        let mut index: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        let index = index.as_object_mut().unwrap();
        index.remove("generation");
        for bundle in index["bundles"].as_object_mut().unwrap().values_mut() {
            let bundle = bundle.as_object_mut().unwrap();
            bundle.remove("pinned");
            let release = serde_json::to_value(release_of(&upload, published).unwrap()).unwrap();
            bundle["releases"].as_array_mut().unwrap().insert(0, release);
        }
        let bytes = serde_json::to_vec_pretty(&index).unwrap();
        assert!(matches!(t.put(INDEX_FILE, &bytes, "application/json", INDEX_CACHE, None).unwrap(), Written::Done));
        t.put(&Release::path_for("shop", &sha_of(&upload)), &upload.wasm, "application/wasm", BUNDLE_CACHE, None).unwrap();
    }

    fn sha_of(u: &Upload) -> String {
        remote_bundle::content_hash(&u.wasm)
    }

    /// Regression: an older CLI rewriting the index dropped `generation`,
    /// so the next write here counted up from 0 again, and every answer it
    /// made was refused as older than the stored one — the precomputed
    /// answers, which apps prefer to the index, stayed stale for good. Now
    /// a write that finds generation 0 resumes past the newest stored
    /// answer, and `resolve_registered` (`idealyst ota resolve`) does the
    /// same before rewriting the answers.
    #[test]
    fn regression_an_older_cli_writing_the_index_leaves_answers_stale() {
        // `idealyst ota resolve` after the older CLI's publish.
        let dir = tempfile::tempdir().unwrap();
        let t = Target::Dir(dir.path().into());
        let app = manifest(false);
        register(&t, &app, ManifestSource::Build, None, None).unwrap();
        for n in 1..=3 {
            publish(&t, &[bundle(n)], u64::from(n), "cli:me").unwrap();
        }
        let before = answer(&t, &app).unwrap();
        assert_eq!((before.generation, before.bundles["shop"].release.as_ref().unwrap().version.as_str()), (3, "1.0.3"));
        publish_with_0_1(&t, bundle(4), 4);
        assert_eq!(read_index(&t).unwrap().generation, 0, "the older CLI dropped it");
        assert_eq!(answer(&t, &app).unwrap(), before, "and rewrote no answer");
        let r = resolve_registered(&t).unwrap();
        assert_eq!((r.written, r.current, r.failed.len()), (1, 0, 0));
        let after = answer(&t, &app).unwrap();
        assert_eq!(after.bundles["shop"].release.as_ref().unwrap().version, "1.0.4");
        assert!(after.generation > before.generation);
        assert_eq!(after.generation, read_index(&t).unwrap().generation);

        // The next publish here, after the older CLI's.
        let dir = tempfile::tempdir().unwrap();
        let t = Target::Dir(dir.path().into());
        register(&t, &app, ManifestSource::Build, None, None).unwrap();
        for n in 1..=3 {
            publish(&t, &[bundle(n)], u64::from(n), "cli:me").unwrap();
        }
        publish_with_0_1(&t, bundle(4), 4);
        publish(&t, &[bundle(5)], 5, "cli:me").unwrap();
        let after = answer(&t, &app).unwrap();
        assert_eq!(after.bundles["shop"].release.as_ref().unwrap().version, "1.0.5");
        assert_eq!((after.generation, read_index(&t).unwrap().generation), (4, 4), "resumed past the stored answer's 3");

        // A location no older CLI touched counts as before.
        let dir = tempfile::tempdir().unwrap();
        let t = Target::Dir(dir.path().into());
        publish(&t, &[bundle(1)], 1, "cli:me").unwrap();
        assert_eq!(read_index(&t).unwrap().generation, 1);
    }
}
