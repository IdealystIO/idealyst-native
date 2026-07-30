//! Flags that were removed with runtime v2 but must still be *parsed*.
//!
//! Sibling of [`crate::core_mode`], which owns the same contract for the
//! core-selection flags (`--new-core` / `--old-core`). The rule is the
//! same in both places: a flag that used to exist and now does nothing
//! must fail with a message naming the migration guide, not with clap's
//! opaque `unexpected argument '--primitives' found`. Keeping the arg
//! declared (and rejecting it here) is what buys the good error.
//!
//! - **`--primitives <list>`** — a hard error carrying the migration
//!   pointer. It selected which `prim-*` primitive families the
//!   generated web wrapper compiled in. Those families were cargo
//!   features on the pre-v2 `runtime-core` (and each backend, and
//!   idea-ui), gating walker dispatch arms, authoring builder fns, and
//!   `Backend` trait methods — none of which exist on runtime v2.
//!   Silently ignoring the flag would be the bad failure: a size-tuned
//!   release pipeline would keep "succeeding" while quietly building
//!   the all-families bundle it was written to avoid, and the operator
//!   would never learn the lever is gone.
//!
//! See `docs/migrating-to-runtime-v2.md`.

use anyhow::Result;

/// The pointer every rejected `--primitives` invocation gets.
const PRIMITIVES_REMOVED: &str = "\
--primitives was removed: per-primitive-family bundle gating no longer \
exists. It selected `prim-*` cargo features on the pre-runtime-v2 core \
(`runtime-core`, each backend crate, and idea-ui), which compiled out \
walker dispatch arms, authoring builder fns, and `Backend` trait \
methods. Runtime v2 has no walker and no `Backend` mega-trait — \
`runtime_vocabulary::handlers::register_builtins` installs one handler \
per primitive into a `runtime_scene::Registry`, and reachability from \
that boot seam (plus LTO) decides what links, so there is nothing for \
the flag to switch off.

Drop --primitives from this invocation; the build it produces is the \
same bundle you already get. Also drop any `default-features = false` \
+ `features = [\"prim-…\"]` selection from your Cargo.toml — those \
features are gone from every crate that had them, and cargo will \
reject the manifest until you do. docs/migrating-to-runtime-v2.md \
explains the successor (per-primitive handler registration) and what \
to write instead.";

/// Validate the removed build flags.
///
/// `primitives` is `idealyst build --primitives <list>` as clap parsed
/// it. Returns `Ok(())` when the build may proceed (i.e. the flag was
/// not passed).
pub fn validate_build_flags(primitives: Option<&[String]>) -> Result<()> {
    if primitives.is_some() {
        anyhow::bail!("{PRIMITIVES_REMOVED}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `--primitives` must fail loudly rather than silently building the
    /// all-families bundle — the flag asks for a size lever that no
    /// longer exists, so a silent no-op would surface as an unexplained
    /// bundle-size regression instead of a command-line error.
    ///
    /// Mirrors `core_mode::tests::old_core_flag_is_a_hard_error_…`.
    #[test]
    fn primitives_flag_is_a_hard_error_pointing_at_the_migration_guide() {
        let list = ["icon".to_string(), "text-input".to_string()];
        let err = validate_build_flags(Some(&list)).unwrap_err().to_string();
        assert!(err.contains("--primitives was removed"), "{err}");
        assert!(err.contains("docs/migrating-to-runtime-v2.md"), "{err}");
    }

    /// An EMPTY list (`--primitives ''` / `--primitives none`) is still
    /// the flag being passed — it used to mean "text/view-only bundle",
    /// the most size-sensitive invocation of all, so it must not slip
    /// through as `Some(&[])`.
    #[test]
    fn primitives_flag_with_an_empty_list_is_still_rejected() {
        assert!(validate_build_flags(Some(&[])).is_err());
    }

    /// The ordinary build: flag absent, nothing to say.
    #[test]
    fn absent_primitives_flag_is_accepted() {
        assert!(validate_build_flags(None).is_ok());
    }
}
