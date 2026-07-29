//! Which core does this build run on? — the runtime-v2 defaults flip.
//!
//! Since the flip, `idealyst dev` / `idealyst build` with no flags run
//! the NEW core (runtime v2) for any project that declares the
//! dual-core convention's `new-core` cargo feature (every `idealyst
//! new` scaffold does). `--old-core` opts a build back onto the old
//! walker; `--new-core` is accepted as a no-op alias so existing
//! invocations keep working. Legacy projects that never declared a
//! `new-core` feature keep building the old core with a one-line note
//! (their source can't compile under the retargeted macro emission
//! until they adopt the convention — see
//! `docs/migrating-to-runtime-v2.md`).
//!
//! One build graph carries exactly one core (proc-macro feature
//! unification — `crates/runtime/macros/Cargo.toml`), so the resolved
//! mode selects the whole wrapper + user-crate feature set:
//! new-core builds compile the user crate `default-features = false,
//! features = ["new-core"]`; old-core builds pin `old-core` when the
//! app declares it (see `build_ios::old_core_user_dep`).

use std::path::Path;

use anyhow::Result;

/// Resolve the effective core for a build of `project_dir`.
///
/// Returns `true` for the new core. `new_core_flag` / `old_core_flag`
/// are the CLI's `--new-core` / `--old-core`.
pub fn resolve(project_dir: &Path, new_core_flag: bool, old_core_flag: bool) -> Result<bool> {
    if new_core_flag && old_core_flag {
        anyhow::bail!(
            "--new-core and --old-core are mutually exclusive (one build graph \
             carries exactly one core)"
        );
    }
    if old_core_flag {
        return Ok(false);
    }
    let dual_core = build_ios::declares_feature(project_dir, "new-core");
    if new_core_flag {
        if !dual_core {
            anyhow::bail!(
                "--new-core requires the dual-core app convention: the project's \
                 Cargo.toml must declare a `new-core` cargo feature (plus the \
                 per-target registration seams). `idealyst new` scaffolds ship it; \
                 for existing projects see docs/migrating-to-runtime-v2.md, or \
                 pass --old-core to stay on the old walker."
            );
        }
        return Ok(true);
    }
    if dual_core {
        Ok(true)
    } else {
        eprintln!(
            "[core] project declares no `new-core` feature — building on the OLD \
             core. Runtime v2 is the default for migrated projects; see \
             docs/migrating-to-runtime-v2.md (silence this note with --old-core)."
        );
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(features: &str) -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("Cargo.toml"),
            format!("[package]\nname = \"demo\"\n\n[features]\n{features}"),
        )
        .unwrap();
        tmp
    }

    /// The runtime-v2 defaults flip: no flags → new core for dual-core
    /// apps, old core (with a note) for legacy apps; `--old-core` opts
    /// out; `--new-core` stays a working alias.
    #[test]
    fn defaults_flip_resolution() {
        let dual = project("new-core = []\nold-core = []\n");
        let legacy = project("robot = []\n");
        assert!(resolve(dual.path(), false, false).unwrap());
        assert!(!resolve(legacy.path(), false, false).unwrap());
        assert!(!resolve(dual.path(), false, true).unwrap());
        assert!(resolve(dual.path(), true, false).unwrap());
        assert!(resolve(legacy.path(), true, false).is_err());
        assert!(resolve(dual.path(), true, true).is_err());
    }
}
