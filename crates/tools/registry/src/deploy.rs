//! Pushing the built registry to S3 and invalidating CloudFront.
//!
//! This shells out to the `aws` CLI rather than linking the AWS SDK. The SDK
//! would add a large dependency tree to a workspace that already takes a long
//! time to build, to do three calls that the CLI already does — and CI and
//! developers both authenticate the CLI the same way, so there is one
//! credential path instead of two.

use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};

/// Index files, `config.json` and `releases.json` must never be served stale:
/// a consumer resolving against a cached index simply cannot see a version
/// that was published a minute ago. `max-age=0, must-revalidate` keeps
/// CloudFront storing the object but forces a revalidation against S3, which
/// is cheap because S3 answers with a 304 on an unchanged ETag.
const INDEX_CACHE: &str = "public, max-age=0, must-revalidate";

/// A `.crate` tarball at a given version is immutable by construction — the
/// version is in the path, and republishing a version is forbidden. Cache it
/// for a year.
const CRATE_CACHE: &str = "public, max-age=31536000, immutable";

pub struct Target {
    pub bucket: String,
    pub distribution_id: Option<String>,
}

fn run(cmd: &mut Command, what: &str) -> Result<()> {
    let out = cmd.output().with_context(|| format!("running {what}"))?;
    if !out.status.success() {
        bail!(
            "{what} failed:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(())
}

/// Upload the tarballs first, then the index.
///
/// Order matters and is not cosmetic: the index is what tells cargo a version
/// exists. If the index landed first, a consumer resolving in that window
/// would find the entry, request the tarball, and get a 404. Publishing the
/// bytes before announcing them makes the window harmless.
pub fn sync(staging: &Path, target: &Target) -> Result<()> {
    let crates_dir = staging.join("crates");
    if crates_dir.exists() {
        run(
            Command::new("aws").args([
                "s3",
                "sync",
                &crates_dir.to_string_lossy(),
                &format!("s3://{}/crates", target.bucket),
                "--cache-control",
                CRATE_CACHE,
                "--content-type",
                "application/x-tar",
                // A published version is immutable, so an object that already
                // exists is already correct — never re-upload it.
                "--size-only",
            ]),
            "aws s3 sync (crates)",
        )?;
    }

    let index_dir = staging.join("index");
    run(
        Command::new("aws").args([
            "s3",
            "sync",
            &index_dir.to_string_lossy(),
            &format!("s3://{}/index", target.bucket),
            "--cache-control",
            INDEX_CACHE,
            "--content-type",
            "text/plain",
        ]),
        "aws s3 sync (index)",
    )?;

    // `index/config.json` went up with the index sync above, but as
    // text/plain. Re-put it with a JSON content type — cargo does not care,
    // but anything else looking at the bucket will.
    for (local, remote) in [
        ("index/config.json", "index/config.json"),
        ("releases.json", "releases.json"),
    ] {
        let p = staging.join(local);
        if !p.exists() {
            continue;
        }
        run(
            Command::new("aws").args([
                "s3",
                "cp",
                &p.to_string_lossy(),
                &format!("s3://{}/{remote}", target.bucket),
                "--cache-control",
                INDEX_CACHE,
                "--content-type",
                "application/json",
            ]),
            &format!("aws s3 cp ({local})"),
        )?;
    }
    Ok(())
}

/// Invalidate the index surface. The `.crate` objects are deliberately left
/// alone — they are new paths, so nothing can be cached for them yet.
pub fn invalidate(target: &Target) -> Result<()> {
    let Some(dist) = &target.distribution_id else {
        return Ok(());
    };
    run(
        Command::new("aws").args([
            "cloudfront",
            "create-invalidation",
            "--distribution-id",
            dist,
            "--paths",
            "/index/*",
            "/config.json",
            "/releases.json",
        ]),
        "aws cloudfront create-invalidation",
    )
}

/// Fetch the registry's `releases.json`, if it has one yet.
pub fn fetch_release_state(target: &Target) -> Result<Option<String>> {
    let out = Command::new("aws")
        .args([
            "s3",
            "cp",
            &format!("s3://{}/releases.json", target.bucket),
            "-",
        ])
        .output()
        .context("running aws s3 cp (releases.json)")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        // A missing object is the normal first-run case, not a failure.
        if err.contains("404") || err.contains("Not Found") || err.contains("NoSuchKey") {
            return Ok(None);
        }
        bail!("could not read s3://{}/releases.json:\n{err}", target.bucket);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).to_string()))
}

/// Fetch one crate's existing index file so new versions append rather than
/// replace. Returns `None` if the crate has never been published.
pub fn fetch_index_file(target: &Target, index_path: &str) -> Result<Option<String>> {
    let out = Command::new("aws")
        .args([
            "s3",
            "cp",
            &format!("s3://{}/index/{index_path}", target.bucket),
            "-",
        ])
        .output()
        .context("running aws s3 cp (index file)")?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).to_string()))
}
