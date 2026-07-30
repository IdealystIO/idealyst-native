//! Core-selection flags — one core, one code path.
//!
//! There is exactly one runtime now: runtime v2 (`runtime-world` +
//! `runtime-scene` + `runtime-vocabulary` over the `runtime-shared`
//! substrate). The pre-v2 walker — the `runtime-core` crate's `Element`
//! enum, `Backend` mega-trait, and render walker — has been deleted, so
//! there is nothing left to resolve *between*: every `idealyst dev` /
//! `idealyst build` / `idealyst run` builds runtime v2 for every target.
//!
//! Two flags survive as compatibility surface, and this module is the
//! single place that interprets them:
//!
//! - **`--new-core`** — a working no-op. It named the default for a
//!   whole release cycle and lives in muscle memory, scripts, and CI
//!   files; accepting it silently keeps those working.
//! - **`--old-core`** — a hard error carrying the migration pointer.
//!   Silently ignoring it would be the bad failure: an invocation that
//!   asks for the old walker's *semantics* (immediate writes, `batch`,
//!   `on_cleanup` in a component body) would build the staged-commit
//!   kernel and diverge at runtime instead of at the command line.
//!
//! Projects no longer declare a core: the `new-core` / `old-core` cargo
//! features are gone from the scaffold and from every in-tree app, and
//! generated wrappers compile the user crate with its plain defaults.
//! See `docs/migrating-to-runtime-v2.md`.

use anyhow::Result;

/// The pointer every rejected `--old-core` invocation gets.
const OLD_CORE_REMOVED: &str = "\
--old-core was removed: the pre-runtime-v2 walker (the `runtime-core` \
crate — `Element`, the `Backend` trait, the render walker) no longer \
exists, so there is no old core to build.

Runtime v2 is the only runtime and needs no flag. Drop --old-core from \
this invocation. If the project's sources were written against the old \
core's semantics (immediate signal writes, `batch(..)`, \
`update(|v: &mut T| ..)`, `on_cleanup` in a component body, creating \
reactive state inside an event handler), port them first — \
docs/migrating-to-runtime-v2.md has the full table of breaking changes \
and the failure mode for each.";

/// Validate the surviving core flags.
///
/// `new_core_flag` / `old_core_flag` are the CLI's `--new-core` /
/// `--old-core`. Returns `Ok(())` when the build may proceed (which is
/// every case except `--old-core`).
pub fn validate_flags(new_core_flag: bool, old_core_flag: bool) -> Result<()> {
    if old_core_flag {
        anyhow::bail!("{OLD_CORE_REMOVED}");
    }
    // `--new-core` asks for what it already gets. Deliberately silent:
    // a note here would fire on every legacy CI invocation with nothing
    // for the operator to do about it.
    let _ = new_core_flag;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--old-core` must fail loudly rather than silently building
    /// runtime v2 — the flag asks for semantics that no longer exist,
    /// so a silent downgrade would surface as runtime divergence
    /// instead of a command-line error.
    #[test]
    fn old_core_flag_is_a_hard_error_pointing_at_the_migration_guide() {
        let err = validate_flags(false, true).unwrap_err().to_string();
        assert!(err.contains("--old-core was removed"), "{err}");
        assert!(err.contains("docs/migrating-to-runtime-v2.md"), "{err}");
    }

    /// `--new-core` stays a working no-op alias so existing scripts,
    /// CI files, and muscle memory keep working.
    #[test]
    fn new_core_flag_is_an_accepted_no_op() {
        assert!(validate_flags(true, false).is_ok());
        assert!(validate_flags(false, false).is_ok());
    }

    /// Both flags together: the removal error wins — it is the
    /// actionable half.
    #[test]
    fn old_core_wins_when_both_flags_are_passed() {
        assert!(validate_flags(true, true).is_err());
    }
}
