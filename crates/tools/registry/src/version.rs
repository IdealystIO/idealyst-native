//! Deciding each crate's next version from the commits that touched it.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

/// What the registry believes it last published, keyed by crate name.
///
/// This lives beside the index in the bucket as `releases.json`. It is OUR
/// bookkeeping, not part of cargo's index schema — cargo never reads it. It
/// exists because an index entry records a version but not the commit that
/// produced it, and "which commits have touched this crate since its last
/// release?" is the whole question a per-crate bump has to answer.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct ReleaseState {
    pub crates: BTreeMap<String, Released>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Released {
    pub version: String,
    /// Commit the release was cut from.
    pub commit: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bump {
    None,
    Patch,
    Minor,
    Major,
}

impl Bump {
    pub fn label(self) -> &'static str {
        match self {
            Bump::None => "—",
            Bump::Patch => "patch",
            Bump::Minor => "minor",
            Bump::Major => "major",
        }
    }

    pub fn apply(self, v: &semver::Version) -> semver::Version {
        match self {
            Bump::None => v.clone(),
            Bump::Patch => semver::Version::new(v.major, v.minor, v.patch + 1),
            Bump::Minor => semver::Version::new(v.major, v.minor + 1, 0),
            Bump::Major => semver::Version::new(v.major + 1, 0, 0),
        }
    }
}

/// Classify one commit message under the Conventional Commits convention this
/// repo already follows (`feat(mcp): …`, `fix(table): …`, `chore: …`).
///
/// Anything unrecognised counts as a patch rather than nothing: a commit that
/// changed a crate's files but used a free-form subject still changed the
/// crate, and silently not republishing it would ship a registry that
/// disagrees with the source.
pub fn classify(message: &str) -> Bump {
    let subject = message.lines().next().unwrap_or("").trim();
    if message
        .lines()
        .skip(1)
        .any(|l| l.starts_with("BREAKING CHANGE:") || l.starts_with("BREAKING-CHANGE:"))
    {
        return Bump::Major;
    }
    let Some(colon) = subject.find(':') else {
        return Bump::Patch;
    };
    let (head, _) = subject.split_at(colon);
    let head = head.trim();
    if head.ends_with('!') {
        return Bump::Major;
    }
    // Strip an optional `(scope)` to get at the bare type.
    let ty = head.split('(').next().unwrap_or(head).trim();
    match ty {
        "feat" => Bump::Minor,
        // `revert` can undo anything; treat it as the strongest non-breaking
        // signal so a reverted feature does not ship as a patch.
        "revert" => Bump::Minor,
        _ => Bump::Patch,
    }
}

/// The bump a crate has earned, from the commits touching its directory since
/// `since` (exclusive).
///
/// Callers handle the never-published case themselves: a first release ships
/// at the version already in the manifest rather than bumping past it, so
/// that the registry starts where the git tags left off instead of skipping a
/// version for no reason.
pub fn bump_for(root: &Path, rel_dir: &str, since: &str) -> Result<Bump> {
    // `%x1e` (record separator) between commits: commit bodies contain blank
    // lines, so no newline-based delimiter is safe here.
    let out = Command::new("git")
        .args([
            "log",
            "--no-merges",
            "--format=%B%x1e",
            &format!("{since}..HEAD"),
            "--",
            rel_dir,
        ])
        .current_dir(root)
        .output()
        .context("running `git log`")?;
    if !out.status.success() {
        // An unknown `since` commit means history was rewritten or the state
        // file is from another repo. Refuse rather than silently republishing
        // everything at a patch bump.
        bail!(
            "`git log {since}..HEAD -- {rel_dir}` failed — is {since} still in this history?\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text
        .split('\u{1e}')
        .filter(|c| !c.trim().is_empty())
        .map(classify)
        .max()
        .unwrap_or(Bump::None))
}

pub fn head_commit(root: &Path) -> Result<String> {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .context("running `git rev-parse HEAD`")?;
    if !out.status.success() {
        bail!("could not resolve HEAD");
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Is the working tree clean? A release cut from a dirty tree records a commit
/// that does not describe the bytes actually published.
pub fn is_clean(root: &Path) -> Result<bool> {
    let out = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(root)
        .output()
        .context("running `git status`")?;
    Ok(out.stdout.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conventional_types_map_to_bumps() {
        assert_eq!(classify("feat(mcp): one catalog across every project"), Bump::Minor);
        assert_eq!(classify("fix(table): row hover must reach cells"), Bump::Patch);
        assert_eq!(classify("chore: bump version"), Bump::Patch);
        assert_eq!(classify("docs: explain the seam"), Bump::Patch);
    }

    #[test]
    fn breaking_changes_are_major() {
        assert_eq!(classify("feat!: drop Element::External"), Bump::Major);
        assert_eq!(classify("refactor(scene)!: rename Host"), Bump::Major);
        assert_eq!(
            classify("feat: new registry\n\nBREAKING CHANGE: the old seam is gone"),
            Bump::Major
        );
    }

    /// A commit that changed a crate but used a free-form subject still
    /// changed it. Classifying it as "no release" would ship a registry that
    /// disagrees with the source.
    #[test]
    fn unconventional_subjects_still_earn_a_patch() {
        assert_eq!(classify("wip: next phase of migrations"), Bump::Patch);
        assert_eq!(classify("tidy up the scroll math"), Bump::Patch);
    }

    #[test]
    fn bumps_apply_semver() {
        let v = semver::Version::new(1, 5, 2);
        assert_eq!(Bump::Patch.apply(&v).to_string(), "1.5.3");
        assert_eq!(Bump::Minor.apply(&v).to_string(), "1.6.0");
        assert_eq!(Bump::Major.apply(&v).to_string(), "2.0.0");
        assert_eq!(Bump::None.apply(&v).to_string(), "1.5.2");
    }

    /// The strongest signal across a crate's commits wins — one `feat:` in a
    /// run of `fix:`es is still a minor release.
    #[test]
    fn the_strongest_signal_wins() {
        let msgs = ["fix: a", "feat: b", "chore: c"];
        assert_eq!(msgs.iter().map(|m| classify(m)).max().unwrap(), Bump::Minor);
    }
}
