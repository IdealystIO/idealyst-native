//! Keeping a generated wrapper's inputs stable between runs.
//!
//! A platform wrapper (`<app>/target/idealyst/<app>/ios/wrapper`, …) is
//! its own cargo workspace with its own `Cargo.lock`, regenerated on
//! every `idealyst dev` / `build` / `run`. Two things decide whether a
//! regeneration costs the user anything:
//!
//! - **What cargo resolves.** If the wrapper resolves independently of
//!   the app, it does not build the graph the app locked. With the lock
//!   deleted per run (the old behaviour) every run re-resolved against
//!   the registry, so each framework release — or any crates.io release
//!   anywhere in the tree — became a fresh generation of every crate
//!   above it, compiled next to the previous one. Measured on CrewForge
//!   (2026-10-08): the iOS wrapper's target held two versions of 25
//!   framework crates, the app's own lock a third (`backend-ios-mobile`
//!   1.11.0 / 1.13.0 in the wrapper, 1.14.0 in the app), and a GC
//!   reclaimed 4.2 GB from it. [`seed_wrapper_lockfile`] makes the app's
//!   lock the wrapper's lock.
//! - **Whether files change.** Cargo fingerprints a path package's
//!   sources by mtime, so rewriting the wrapper's `src/lib.rs` with
//!   identical bytes dirties the wrapper crate every run — and for a
//!   `staticlib` that means re-archiving the whole (~1 GB in debug) `.a`.
//!   [`write_if_changed`] writes only on a content change.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Write `contents` to `path` only if the file does not already hold
/// exactly those bytes. Returns whether it wrote.
///
/// Cargo decides a path package is dirty from source mtimes, so an
/// identical rewrite is not free: it recompiles the wrapper crate and
/// re-links its artifact on every run.
pub fn write_if_changed(path: &Path, contents: &str) -> Result<bool> {
    if fs::read(path).is_ok_and(|on_disk| on_disk == contents.as_bytes()) {
        return Ok(false);
    }
    fs::write(path, contents).with_context(|| format!("write {}", path.display()))?;
    Ok(true)
}

/// Name of the marker, next to the wrapper's `Cargo.lock`, recording
/// which app lock the wrapper's lock was last seeded from.
pub const LOCK_SEED_MARKER: &str = ".idealyst-lock-seed";

/// What [`seed_wrapper_lockfile`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockSeed {
    /// The app's lock was copied in (first run, or the app's lock changed
    /// since the last seed).
    Seeded { from: PathBuf },
    /// The app's lock is the one the wrapper was last seeded from; the
    /// wrapper's lock (which cargo has since extended with the wrapper's
    /// own entries) was left alone.
    Unchanged,
    /// The app has no `Cargo.lock` to follow; the wrapper keeps its own.
    NoAppLock,
}

/// Make the wrapper resolve exactly the dependency graph the app locked.
///
/// The app's `Cargo.lock` (the first one walking up from `project_dir` —
/// the workspace root's, for a workspace member) is copied over the
/// wrapper's whenever its CONTENT differs from the lock the wrapper was
/// last seeded from. Cargo keeps every locked version that still
/// satisfies a requirement, so the wrapper then builds the same version
/// of every crate the app builds; it adds only the wrapper's own entries
/// (its root package, and any dependency the app's lock lacks) and prunes
/// the app-only ones. Between app-lock changes the wrapper's lock is left
/// untouched, so a run does no resolution at all and the wrapper's own
/// additions stay pinned too.
///
/// # Why content, not mtime
///
/// The catalog wrapper copies when the app's lock is *newer*. A checkout,
/// a `git stash pop` or a copy across machines can move the app's lock
/// back in time with different content, and an mtime test would then
/// keep a wrapper lock the app no longer has. The marker records a hash
/// of the bytes that were seeded.
///
/// # Why this replaced deleting the lock
///
/// [`crate::refresh_wrapper_lockfile`] deleted the wrapper's lock every
/// run so a stale one could not pin the wrapper behind the app. That
/// fixed the staleness but made the wrapper resolve FRESH every run —
/// newest-compatible from the registry, not what the app locked — which
/// is both a different graph from the app's and a new generation of every
/// crate above any newly published one. Seeding fixes the staleness the
/// same way (the wrapper follows every app-lock change) without the
/// re-resolution.
///
/// Only for wrappers with a PRIVATE target dir. Seeding makes the
/// wrapper's units hash like the app's; a wrapper that builds into the
/// app's own `target/` would then interleave its units with the app's,
/// which is the duplicate-crate failure the Linux builder hit (see
/// `refresh_wrapper_lockfile`).
pub fn seed_wrapper_lockfile(wrapper_dir: &Path, project_dir: &Path) -> Result<LockSeed> {
    let Some(app_lock) = project_dir
        .ancestors()
        .map(|d| d.join("Cargo.lock"))
        .find(|p| p.is_file())
    else {
        return Ok(LockSeed::NoAppLock);
    };
    let bytes = fs::read(&app_lock).with_context(|| format!("read {}", app_lock.display()))?;
    let marker = format!("{} {}\n", content_id(&bytes), app_lock.display());
    let marker_path = wrapper_dir.join(LOCK_SEED_MARKER);
    let wrapper_lock = wrapper_dir.join("Cargo.lock");
    if let (Ok(recorded), Ok(current)) = (fs::read_to_string(&marker_path), fs::read(&wrapper_lock))
    {
        let mut lines = recorded.lines();
        let same_seed = lines.next().is_some_and(|l| format!("{l}\n") == marker);
        // The wrapper's lock must still be the one this seed produced:
        // either the seed itself (cargo has not run yet), or what cargo
        // made of it ([`record_resolved_lock`]). Anything else — an older
        // CLI that deleted and re-resolved it, a hand-run `cargo update`
        // in the wrapper — is re-seeded rather than trusted.
        let current = content_id(&current);
        let ours = match lines.next() {
            Some(resolved) => resolved == current,
            None => current == content_id(&bytes),
        };
        if same_seed && ours {
            return Ok(LockSeed::Unchanged);
        }
    }
    fs::write(&wrapper_lock, &bytes).with_context(|| {
        format!(
            "seed {} from the app's {}",
            wrapper_lock.display(),
            app_lock.display()
        )
    })?;
    // Marker AFTER the lock: a crash between the two re-seeds next run
    // rather than trusting a lock that was never written.
    fs::write(&marker_path, marker).with_context(|| format!("write {}", marker_path.display()))?;
    Ok(LockSeed::Seeded { from: app_lock })
}

/// Record the wrapper's lock as cargo left it after a build of the seeded
/// wrapper, so the next [`seed_wrapper_lockfile`] can tell "cargo
/// completed our seed" from "something else rewrote the lock". Call it
/// after the wrapper's cargo build EXITS, whether or not it succeeded —
/// cargo writes the lock before compiling, so a failed compile still
/// leaves the resolved lock to keep.
///
/// A no-op without a seed marker (no app lock to follow).
pub fn record_resolved_lock(wrapper_dir: &Path) -> Result<()> {
    let marker_path = wrapper_dir.join(LOCK_SEED_MARKER);
    let Ok(recorded) = fs::read_to_string(&marker_path) else {
        return Ok(());
    };
    let Some(seed) = recorded.lines().next() else {
        return Ok(());
    };
    let Ok(lock) = fs::read(wrapper_dir.join("Cargo.lock")) else {
        return Ok(());
    };
    let updated = format!("{seed}\n{}\n", content_id(&lock));
    if updated != recorded {
        fs::write(&marker_path, updated)
            .with_context(|| format!("write {}", marker_path.display()))?;
    }
    Ok(())
}

/// Stable content id: FNV-1a 64 plus the length. Not a security hash — it
/// only has to notice that a lock changed, and it must give the same
/// answer across CLI rebuilds (std's `DefaultHasher` does not promise
/// that).
fn content_id(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("fnv1a64:{h:016x}:{}", bytes.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn age(path: &Path, secs: u64) {
        let t = SystemTime::now() - Duration::from_secs(secs);
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    #[test]
    fn write_if_changed_leaves_identical_content_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f");
        assert!(write_if_changed(&p, "a").unwrap());
        age(&p, 3600);
        let before = fs::metadata(&p).unwrap().modified().unwrap();
        assert!(!write_if_changed(&p, "a").unwrap());
        assert_eq!(
            fs::metadata(&p).unwrap().modified().unwrap(),
            before,
            "mtime moved"
        );
        assert!(write_if_changed(&p, "b").unwrap());
        assert_eq!(fs::read_to_string(&p).unwrap(), "b");
    }

    #[test]
    fn seed_copies_the_app_lock_and_then_leaves_cargos_additions_alone() {
        let tmp = tempfile::tempdir().unwrap();
        let ws = tmp.path().join("ws");
        let app = ws.join("crates/app");
        let wrapper = tmp.path().join("wrapper");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&wrapper).unwrap();
        // A workspace member: the lock is at the workspace root.
        fs::write(ws.join("Cargo.lock"), "app lock v1").unwrap();

        assert!(matches!(
            seed_wrapper_lockfile(&wrapper, &app).unwrap(),
            LockSeed::Seeded { .. }
        ));
        assert_eq!(
            fs::read_to_string(wrapper.join("Cargo.lock")).unwrap(),
            "app lock v1"
        );

        // Not yet built: the seed itself is ours.
        assert_eq!(
            seed_wrapper_lockfile(&wrapper, &app).unwrap(),
            LockSeed::Unchanged
        );

        // Cargo extends the seeded lock with the wrapper's own entries.
        fs::write(wrapper.join("Cargo.lock"), "app lock v1 + wrapper entries").unwrap();
        record_resolved_lock(&wrapper).unwrap();
        assert_eq!(
            seed_wrapper_lockfile(&wrapper, &app).unwrap(),
            LockSeed::Unchanged
        );
        assert_eq!(
            fs::read_to_string(wrapper.join("Cargo.lock")).unwrap(),
            "app lock v1 + wrapper entries",
            "an unchanged app lock must not clobber the wrapper's resolved lock"
        );

        // The app's lock moves — even BACKWARDS in time — and the wrapper follows.
        fs::write(ws.join("Cargo.lock"), "app lock v2").unwrap();
        age(&ws.join("Cargo.lock"), 86_400);
        assert!(matches!(
            seed_wrapper_lockfile(&wrapper, &app).unwrap(),
            LockSeed::Seeded { .. }
        ));
        assert_eq!(
            fs::read_to_string(wrapper.join("Cargo.lock")).unwrap(),
            "app lock v2"
        );
    }

    /// Something other than this CLI rewrote the wrapper's lock (an older
    /// CLI that deletes and re-resolves it — the bind-mounted CrewForge
    /// tree is built from a Mac and a devcontainer — or a hand-run
    /// `cargo update` in the wrapper). The drifted lock must not be
    /// trusted just because the app's lock is unchanged.
    #[test]
    fn a_lock_rewritten_behind_our_back_is_reseeded() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        let wrapper = tmp.path().join("wrapper");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&wrapper).unwrap();
        fs::write(app.join("Cargo.lock"), "app lock").unwrap();
        seed_wrapper_lockfile(&wrapper, &app).unwrap();
        fs::write(wrapper.join("Cargo.lock"), "app lock + wrapper entries").unwrap();
        record_resolved_lock(&wrapper).unwrap();

        fs::write(
            wrapper.join("Cargo.lock"),
            "re-resolved against the registry",
        )
        .unwrap();
        assert!(matches!(
            seed_wrapper_lockfile(&wrapper, &app).unwrap(),
            LockSeed::Seeded { .. }
        ));
        assert_eq!(
            fs::read_to_string(wrapper.join("Cargo.lock")).unwrap(),
            "app lock"
        );
    }

    #[test]
    fn a_missing_wrapper_lock_is_reseeded_even_with_a_matching_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        let wrapper = tmp.path().join("wrapper");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&wrapper).unwrap();
        fs::write(app.join("Cargo.lock"), "lock").unwrap();
        seed_wrapper_lockfile(&wrapper, &app).unwrap();
        fs::remove_file(wrapper.join("Cargo.lock")).unwrap();
        assert!(matches!(
            seed_wrapper_lockfile(&wrapper, &app).unwrap(),
            LockSeed::Seeded { .. }
        ));
        assert!(wrapper.join("Cargo.lock").is_file());
    }

    #[test]
    fn no_app_lock_keeps_the_wrappers_own() {
        let tmp = tempfile::tempdir().unwrap();
        let app = tmp.path().join("app");
        let wrapper = tmp.path().join("wrapper");
        fs::create_dir_all(&app).unwrap();
        fs::create_dir_all(&wrapper).unwrap();
        fs::write(wrapper.join("Cargo.lock"), "wrapper's own").unwrap();
        // No Cargo.lock anywhere above `app` inside the tempdir; a stray one
        // above the tempdir would make this test meaningless, so check.
        if app.ancestors().any(|d| d.join("Cargo.lock").is_file()) {
            return;
        }
        assert_eq!(
            seed_wrapper_lockfile(&wrapper, &app).unwrap(),
            LockSeed::NoAppLock
        );
        assert_eq!(
            fs::read_to_string(wrapper.join("Cargo.lock")).unwrap(),
            "wrapper's own"
        );
    }
}
