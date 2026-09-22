//! Parser → descriptor → diff → applier, closed.
//!
//! For every pair in [`ui_lowering_parity::edits`] this asserts
//!
//! ```text
//! Element(original) + apply(diff(desc(original), desc(edited))) == Element(edited)
//! ```
//!
//! on all three projections of a real mount — the op sequence, every
//! recorded capability call, and the final scene. `desc()` is the
//! build-time producer the CLI runs; the two `Element`s are what the
//! compiler produced from the two bodies. Nothing here is a dev server,
//! and nothing is mocked: every stage of the real path participates.
//!
//! What this catches that no smaller test can: a diff that produces a
//! patch the applier cannot honour, an applier that honours it
//! differently than the compiler would have, and a descriptor whose node
//! numbers address the wrong element. Each half is internally consistent
//! on its own; only the round trip pins them to each other.
//!
//! The refusal pairs are half the corpus and matter as much. An edit
//! that must be refused silently becoming one that is accepted-and-wrong
//! is the failure this system has to make impossible.

#![cfg(feature = "ui-overlay")]

use runtime_macros_parse::{describe, Ui};
use runtime_template::{diff, Descriptor, SiteId};
use runtime_vocabulary::overlay;
use ui_lowering_parity::{edits, Mode};

/// The descriptor for one body, under a site id shared by both halves of
/// a pair — a diff is only defined for two versions of the SAME site,
/// and a pair is exactly that.
fn desc(body: &str) -> Descriptor {
    let mut ui: Ui = syn::parse_str(body).expect("pair body re-parses");
    describe(
        SiteId {
            package: "ui-lowering-parity".into(),
            file: "edits".into(),
            line: 1,
            col: 1,
        },
        &mut ui,
    )
    .expect("the stamping walk and the describing walk agree")
}

/// The site key the ORIGINAL's compiled tags carry.
///
/// Read off a built tree rather than computed: every pair's `ui!` is
/// written inside one `macro_rules!` body, so they all share a call span
/// and therefore a key. That is fine here — the suite resets staging
/// between cases — but it does mean the key has to come from the
/// expansion rather than from a path.
fn site_key_of(recording_tags: &[(Option<runtime_scene::NodeTag>, runtime_scene::NodeTag)]) -> u64 {
    recording_tags
        .iter()
        .find(|(parent, _)| parent.is_none())
        .map(|(_, t)| t.site)
        .expect("the original must have a tagged root")
}

#[test]
fn a_patched_original_renders_exactly_like_the_edited_source() {
    for pair in edits::all() {
        if !pair.patchable {
            continue;
        }
        let patch = match diff(&desc(pair.original_body), &desc(pair.edited_body)) {
            Ok(p) => p,
            Err(e) => panic!("{}: expected a patch, got a refusal: {e}", pair.name),
        };
        assert!(!patch.edits.is_empty(), "{}: the diff found nothing to change", pair.name);

        for &mode in Mode::ALL {
            overlay::reset();
            // The site key lives in the compiled tags, so it takes one
            // unpatched build to learn it.
            let site = site_key_of(&(pair.original)(mode).tags);

            overlay::reset();
            overlay::stage_key(site, patch.edits.to_vec());
            let patched = (pair.original)(mode);

            overlay::reset();
            let expected = (pair.edited)(mode);

            assert_eq!(
                patched.structural(),
                expected.structural(),
                "{} [{}]: structural projection",
                pair.name,
                mode.suffix()
            );
            assert_eq!(
                patched.scene,
                expected.scene,
                "{} [{}]: final scene",
                pair.name,
                mode.suffix()
            );
            assert_eq!(
                patched.full(),
                expected.full(),
                "{} [{}]: full op projection",
                pair.name,
                mode.suffix()
            );
        }
    }
    overlay::reset();
}

/// An edit that changes compiled code must be REFUSED by the diff, with
/// a reason. Accepting one would mean applying a patch built on an
/// assumption the running binary does not satisfy.
#[test]
fn an_edit_that_changes_code_is_refused_with_a_reason() {
    let mut checked = 0;
    for pair in edits::all() {
        if pair.patchable {
            continue;
        }
        match diff(&desc(pair.original_body), &desc(pair.edited_body)) {
            Ok(patch) => panic!(
                "{}: expected a refusal, got {} edit(s): {:?}",
                pair.name,
                patch.edits.len(),
                patch.edits
            ),
            Err(e) => {
                assert!(!e.to_string().is_empty(), "{}: a refusal must say why", pair.name);
                checked += 1;
            }
        }
    }
    assert!(checked > 0, "no refusal pairs in the corpus");
}

/// A site diffed against itself produces no edits. Without this, a diff
/// that emitted a spurious `SetProp` for every node would still pass the
/// round trip above — the patch would just happen to write the values
/// that were already there.
#[test]
fn diffing_a_site_against_itself_finds_nothing() {
    for pair in edits::all() {
        let d = desc(pair.original_body);
        let patch = diff(&d, &d).unwrap_or_else(|e| panic!("{}: {e}", pair.name));
        assert!(
            patch.edits.is_empty(),
            "{}: diffing a descriptor against itself produced {:?}",
            pair.name,
            patch.edits
        );
    }
}

/// Pair names must be unique — two pairs sharing one would make a
/// failure message point at the wrong case.
#[test]
fn pair_names_are_unique() {
    let mut names: Vec<&str> = edits::all().iter().map(|p| p.name).collect();
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(before, names.len(), "duplicate pair name");
}
