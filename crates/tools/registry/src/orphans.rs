//! Versions the index serves that `releases.json` never recorded.
//!
//! A publish uploads each crate (tarball, then index line) before packaging
//! the next, and writes `releases.json` only once every crate is up. A run
//! that stops partway leaves its uploads live and unrecorded. The planner,
//! reading `releases.json`, then picks the same version numbers again, and the
//! publish refuses them because the content differs.
//!
//! Such a version cannot be withdrawn: the index has served it, so a
//! consumer's `cargo update` may already have resolved it. The planner adopts
//! it as the crate's last release instead — the version the index serves, at
//! the commit its tarball embeds (`.cargo_vcs_info.json`) — and change
//! detection, the API check and the bump all run from there. The next release
//! of the crate continues above the orphan, at the level the commits since it
//! call for.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::version::{ReleaseState, Released};

/// A version adopted from the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Orphan {
    pub name: String,
    /// What `releases.json` recorded, if anything.
    pub recorded: Option<String>,
    pub version: semver::Version,
    pub commit: String,
}

/// The highest non-yanked version in an index file.
pub fn latest_version(index_body: &str) -> Result<Option<semver::Version>> {
    let mut best: Option<semver::Version> = None;
    for line in index_body.lines().filter(|l| !l.trim().is_empty()) {
        let v: serde_json::Value = serde_json::from_str(line).context("parsing an index line")?;
        if v.get("yanked").and_then(|y| y.as_bool()) == Some(true) {
            continue;
        }
        let vers = v.get("vers").and_then(|x| x.as_str()).context("index line without `vers`")?;
        let vers = semver::Version::parse(vers).with_context(|| format!("parsing index version {vers}"))?;
        if best.as_ref().is_none_or(|b| vers > *b) {
            best = Some(vers);
        }
    }
    Ok(best)
}

/// Record in `state` every crate whose index serves a version newer than the
/// one recorded (or any version, for a crate `releases.json` lacks).
///
/// `latest` is each crate's highest live version in the index, `commit_of`
/// reads the commit a published tarball was cut from, and `commit_known` says
/// whether this checkout has that commit. An orphan cut from a commit this
/// checkout lacks is an error: its change detection cannot run here, the same
/// case `--skip` exists for.
pub fn adopt(
    state: &mut ReleaseState,
    latest: &BTreeMap<String, semver::Version>,
    mut commit_of: impl FnMut(&str, &semver::Version) -> Result<String>,
    commit_known: impl Fn(&str) -> bool,
) -> Result<Vec<Orphan>> {
    let mut adopted = Vec::new();
    for (name, version) in latest {
        let recorded = state.crates.get(name).map(|r| r.version.clone());
        if let Some(rec) = &recorded {
            let rec = semver::Version::parse(rec).with_context(|| format!("parsing recorded version of {name}"))?;
            if *version <= rec {
                continue;
            }
        }
        let commit = commit_of(name, version)?;
        if !commit_known(&commit) {
            bail!(
                "{name} {version} is in the index but not in releases.json (a publish stopped \
                 partway), and it was cut from {commit}, which this checkout does not have. \
                 Fetch that commit, or leave {name} out with --skip {name}."
            );
        }
        state
            .crates
            .insert(name.clone(), Released { version: version.to_string(), commit: commit.clone() });
        adopted.push(Orphan { name: name.clone(), recorded, version: version.clone(), commit });
    }
    Ok(adopted)
}

/// The commit a `.crate` was packaged from (`git.sha1` in the
/// `.cargo_vcs_info.json` cargo writes into every tarball).
pub fn vcs_commit(crate_file: &Path) -> Result<String> {
    let f = std::fs::File::open(crate_file).with_context(|| format!("opening {}", crate_file.display()))?;
    let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(f));
    for entry in ar.entries().context("reading .crate tarball")? {
        let mut entry = entry?;
        let path = entry.path()?.to_path_buf();
        if path.file_name().is_some_and(|f| f == ".cargo_vcs_info.json") && path.components().count() == 2 {
            let mut s = String::new();
            std::io::Read::read_to_string(&mut entry, &mut s)?;
            let v: serde_json::Value = serde_json::from_str(&s).context("parsing .cargo_vcs_info.json")?;
            return v
                .pointer("/git/sha1")
                .and_then(|x| x.as_str())
                .map(str::to_owned)
                .context(".cargo_vcs_info.json has no git.sha1");
        }
    }
    bail!("{} has no .cargo_vcs_info.json (packaged outside git?)", crate_file.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(vers: &str, yanked: bool) -> String {
        format!(r#"{{"name":"x","vers":"{vers}","deps":[],"cksum":"0","features":{{}},"yanked":{yanked}}}"#)
    }

    fn state(entries: &[(&str, &str, &str)]) -> ReleaseState {
        let mut s = ReleaseState::default();
        for (n, v, c) in entries {
            s.crates.insert(n.to_string(), Released { version: v.to_string(), commit: c.to_string() });
        }
        s
    }

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    #[test]
    fn latest_version_skips_yanked_and_compares_semver() {
        let body = [line("1.9.0", false), line("1.10.0", false), line("2.0.0", true)].join("\n");
        assert_eq!(latest_version(&body).unwrap(), Some(v("1.10.0")));
        assert_eq!(latest_version("").unwrap(), None);
    }

    /// REGRESSION: a publish that stopped partway left `server-macros 2.0.0`
    /// (and 17 more) in the index while `releases.json` still said 1.5.2.
    /// The planner re-picked 2.0.0 and the publish refused it as already
    /// published with different content. The orphan becomes the recorded
    /// release, so the next bump goes above it.
    #[test]
    fn regression_an_unrecorded_published_version_becomes_the_last_release() {
        let mut s = state(&[("server-macros", "1.5.2", "old"), ("runtime-shared", "3.1.0", "old")]);
        let latest = BTreeMap::from([
            ("server-macros".to_string(), v("2.0.0")),
            ("runtime-shared".to_string(), v("3.2.0")),
        ]);
        let got = adopt(&mut s, &latest, |_, _| Ok("orphan-sha".into()), |_| true).unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(s.crates["server-macros"].version, "2.0.0");
        assert_eq!(s.crates["server-macros"].commit, "orphan-sha");
        let rs = got.iter().find(|o| o.name == "runtime-shared").unwrap();
        assert_eq!(rs.recorded.as_deref(), Some("3.1.0"));
        // A minor from the adopted version clears the orphan.
        let next = crate::version::Bump::Minor.apply(&v(&s.crates["runtime-shared"].version));
        assert_eq!(next, v("3.3.0"));
    }

    #[test]
    fn recorded_versions_are_left_alone_and_tarballs_unread() {
        let mut s = state(&[("a", "1.2.0", "rec")]);
        let latest = BTreeMap::from([("a".to_string(), v("1.2.0"))]);
        let got = adopt(&mut s, &latest, |_, _| panic!("no orphan, no tarball read"), |_| true).unwrap();
        assert!(got.is_empty());
        assert_eq!(s.crates["a"].commit, "rec");
    }

    /// A crate `releases.json` never recorded (an initial publish that was
    /// cut off before the state file was written) is adopted too.
    #[test]
    fn an_unrecorded_initial_publish_is_adopted() {
        let mut s = ReleaseState::default();
        let latest = BTreeMap::from([("new".to_string(), v("0.1.0"))]);
        let got = adopt(&mut s, &latest, |_, _| Ok("sha".into()), |_| true).unwrap();
        assert_eq!(got[0].recorded, None);
        assert_eq!(s.crates["new"].version, "0.1.0");
    }

    #[test]
    fn an_orphan_from_an_unknown_commit_is_refused() {
        let mut s = state(&[("a", "1.0.0", "rec")]);
        let latest = BTreeMap::from([("a".to_string(), v("1.1.0"))]);
        let err = adopt(&mut s, &latest, |_, _| Ok("gone".into()), |_| false).unwrap_err();
        assert!(err.to_string().contains("--skip a"), "{err}");
        assert_eq!(s.crates["a"].version, "1.0.0", "state untouched on refusal");
    }
}
