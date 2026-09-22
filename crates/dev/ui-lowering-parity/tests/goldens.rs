//! Phase-1 gate: the DIRECT lowering against the goldens captured from
//! the PRE-slot-rewrite emitter.
//!
//! These goldens are the whole point of the phase-1 ordering. The
//! slot-list rewrite changes *when* a dynamic prop expression is
//! evaluated (hoisted, in source order, instead of wherever the builder
//! chain happened to splice it), and nothing else may change. A suite
//! that only compared the rewritten lowering against itself would be
//! satisfied by a uniformly-wrong emitter; comparing against a frozen
//! recording of the old one is what makes "behavior-preserving"
//! falsifiable.
//!
//! Regenerate with `UPDATE_UI_PARITY_GOLDENS=1 cargo test -p
//! ui-lowering-parity` — and only after reviewing the diff, because
//! re-baselining discards the pre-rewrite reference permanently.

use ui_lowering_parity::{check_golden, fixtures, Mode};

#[test]
fn direct_lowering_matches_the_frozen_reference() {
    let mut failures: Vec<String> = Vec::new();
    for fixture in fixtures::all() {
        for mode in Mode::ALL {
            let recording = (fixture.direct)(*mode);
            let result = std::panic::catch_unwind(|| {
                check_golden(fixture.name, *mode, &recording)
            });
            if result.is_err() {
                failures.push(format!("{}.{}", fixture.name, mode.suffix()));
            }
        }
    }
    assert!(failures.is_empty(), "goldens differ for: {failures:?}");
}

/// Recording the same fixture twice must produce the same bytes — node
/// ids are creation-order, and a hash-order-dependent emission would
/// make every other assertion in this suite flaky rather than false.
#[test]
fn recordings_are_deterministic() {
    for fixture in fixtures::all() {
        let a = (fixture.direct)(Mode::Spliced);
        let b = (fixture.direct)(Mode::Spliced);
        assert_eq!(a.golden(), b.golden(), "`{}` is not deterministic", fixture.name);
    }
}

/// The corpus must stay a corpus: a fixture dropped by accident would
/// silently shrink the gate, and an unused golden means a fixture was
/// renamed without its reference moving.
#[test]
fn every_fixture_has_a_golden_and_every_golden_a_fixture() {
    let names: Vec<String> = fixtures::all().iter().map(|f| f.name.to_string()).collect();
    assert!(names.len() >= 30, "the corpus must cover every node kind; got {}", names.len());

    let mut on_disk: Vec<String> = std::fs::read_dir(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("goldens"),
    )
    .expect("goldens dir")
    .filter_map(|e| {
        let name = e.ok()?.file_name().to_string_lossy().to_string();
        name.strip_suffix(".golden").map(|s| s.to_string())
    })
    .collect();
    on_disk.sort();
    let mut expected: Vec<String> = names
        .iter()
        .flat_map(|n| Mode::ALL.iter().map(move |m| format!("{n}.{}", m.suffix())))
        .collect();
    expected.sort();
    assert_eq!(expected, on_disk, "fixture list and goldens dir disagree");
}

/// The `ui-overlay` feature must not change what a site BUILDS.
///
/// This is the byte-identity gate, and it is deliberately "same scene"
/// rather than "same tokens": every golden above is checked against the
/// SAME frozen file whichever way the feature is set, so
///
/// ```text
/// cargo test -p ui-lowering-parity
/// cargo test -p ui-lowering-parity --features ui-overlay
/// ```
///
/// both green is the stronger statement — same op sequence, same final
/// scene, same deltas after every drive, across all 49 fixtures in both
/// structural modes. A token diff would also flag the `static`
/// descriptor and the `tag(…)` wrappers, which are exactly the things
/// that are ALLOWED to differ.
///
/// What this test adds is the half a golden cannot see: that the
/// feature is doing something when it is ON. An emission that silently
/// no-opped would pass every golden for the wrong reason.
#[cfg(feature = "ui-overlay")]
#[test]
fn building_a_site_registers_its_descriptor() {
    runtime_vocabulary::overlay::reset();
    let before = runtime_vocabulary::overlay::stats().0;
    let _ = (fixtures::all()[0].direct)(Mode::Spliced);
    let (sites, patches) = runtime_vocabulary::overlay::stats();
    assert!(
        sites > before,
        "with `ui-overlay` on, building a site must register its descriptor"
    );
    assert_eq!(patches, 0, "nothing staged a patch");
}

/// Fixture names must be unique — two fixtures sharing a name would
/// silently overwrite each other's golden.
#[test]
fn fixture_names_are_unique() {
    let mut names: Vec<&str> = fixtures::all().iter().map(|f| f.name).collect();
    let before = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(before, names.len(), "duplicate fixture name");
}
