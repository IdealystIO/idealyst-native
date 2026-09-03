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
    // Leading blank lines are trimmed FIRST. `bump_for` reads commits with
    // `--format=%B%x1e` and splits on the separator, and git emits a newline
    // between records — so every record except the first arrives starting
    // with `\n`. Without this trim, `lines().next()` was `""` for all of
    // them, no colon was found, and they each degraded to `Patch`: only the
    // NEWEST commit touching a crate was ever really classified. A `feat`
    // one commit back therefore shipped as x.y.(z+1) instead of
    // x.(y+1).0 — and a published version is immutable.
    let message = message.trim_start();
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
pub fn bump_for(root: &Path, rel_dir: &str, nested: &[String], since: &str) -> Result<Bump> {
    // `%x1e` (record separator) between commits: commit bodies contain blank
    // lines, so no newline-based delimiter is safe here.
    let args = log_args(rel_dir, nested, since);
    let out = Command::new("git")
        .args(&args)
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

/// Arguments for the "what changed in this crate?" git query.
///
/// Nested workspace members own their own releases, so their files are
/// excluded even though they live under this crate's path. There are 49 such
/// nesting relationships in this workspace — every SDK with an `examples/`
/// crate — so without the exclusions a demo edit republishes the SDK it
/// demonstrates, and a demo under something like `runtime-shared` would make
/// every consumer rebuild most of the framework for a change that is not in
/// the crate at all.
fn log_args(rel_dir: &str, nested: &[String], since: &str) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "log".into(),
        "--no-merges".into(),
        "--format=%B%x1e".into(),
        format!("{since}..HEAD"),
        "--".into(),
        rel_dir.to_string(),
    ];
    args.extend(nested.iter().map(|d| format!(":(exclude){d}")));
    args
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

    /// Regression: `crates/sdk/client/dnd/examples/kanban-demo` is its own
    /// crate, but its files sit under `dnd`'s path — so a comment edit in the
    /// demo planned a release of `dnd`. Nested members are excluded.
    #[test]
    fn regression_nested_members_are_excluded_from_a_crates_changes() {
        let args = log_args(
            "crates/sdk/client/dnd",
            &["crates/sdk/client/dnd/examples/kanban-demo".to_string(),
              "crates/sdk/client/dnd/examples/sortable-demo".to_string()],
            "abc123",
        );
        assert_eq!(args[3], "abc123..HEAD");
        assert_eq!(args[5], "crates/sdk/client/dnd");
        assert_eq!(args[6], ":(exclude)crates/sdk/client/dnd/examples/kanban-demo");
        assert_eq!(args[7], ":(exclude)crates/sdk/client/dnd/examples/sortable-demo");
    }

    #[test]
    fn a_crate_with_no_nested_members_gets_no_exclusions() {
        let args = log_args("crates/css", &[], "abc123");
        assert_eq!(args.len(), 6);
        assert_eq!(args.last().unwrap(), "crates/css");
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

    /// Regression: the records `bump_for` feeds `classify` come from
    /// `git log --format=%B%x1e` split on the separator, and git writes a
    /// newline between records — so every commit but the NEWEST arrived with
    /// a leading blank line, its subject was read as `""`, and it degraded
    /// to `Patch`. `feat(ios): point the robot bridge at the element
    /// registry` was three commits back in `backend-ios-mobile` and planned
    /// as 1.5.3 instead of 1.6.0. Versions are immutable, so this had to be
    /// caught before a publish, not after.
    ///
    /// `the_strongest_signal_wins` above missed it by handing `classify`
    /// clean subjects the split never produces; this one uses the real
    /// shape.
    #[test]
    fn regression_every_commit_in_a_log_split_is_classified_not_just_the_first() {
        // One separator-delimited log, exactly as `bump_for` builds it.
        let log = "fix: newest\n\u{1e}\nfeat: older\n\u{1e}\nchore: oldest\n\u{1e}";
        let strongest = log
            .split('\u{1e}')
            .filter(|c| !c.trim().is_empty())
            .map(classify)
            .max()
            .unwrap();
        assert_eq!(
            strongest,
            Bump::Minor,
            "a feat that is not the newest commit still earns a minor"
        );

        // And the pieces, so a failure says which half broke.
        assert_eq!(classify("\nfeat(ios): reach the element registry"), Bump::Minor);
        assert_eq!(classify("\n\nfeat!: drop Element::External"), Bump::Major);
        assert_eq!(classify("\nfix(table): row hover reaches cells"), Bump::Patch);
    }
}
