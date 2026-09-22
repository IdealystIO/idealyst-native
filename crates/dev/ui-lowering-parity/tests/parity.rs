//! The cross-lowering gate: every fixture, expanded through both
//! `ui_lowered!(direct { … })` and `ui_lowered!(template { … })`, must
//! produce the same scene.
//!
//! Three assertions per fixture per structural mode, checked coarsest
//! first because that is the order in which a divergence is
//! diagnosable:
//!
//! 1. the same structural-op sequence (creation + the 7-method `Host`
//!    seam),
//! 2. the same final scene,
//! 3. the same full op stream, step by step — which is assertion 3 in
//!    the task's phrasing ("after driving every signal the fixture
//!    exposes"), since the recording carries one labelled step per drive.
//!
//! Requires `--features template`.

#![cfg(feature = "template")]

use ui_lowering_parity::{assert_parity, fixtures, Mode};

#[test]
fn every_fixture_builds_the_same_scene_under_both_lowerings() {
    let mut checked = 0usize;
    for fixture in fixtures::all() {
        let template = fixture.template.expect("--features template");
        for mode in Mode::ALL {
            let direct = (fixture.direct)(*mode);
            let templated = template(*mode);
            assert_parity(&format!("{}[{:?}]", fixture.name, mode), &direct, &templated);
            checked += 1;
        }
    }
    assert_eq!(checked, fixtures::all().len() * Mode::ALL.len());
}

/// The template lowering must also still match the FROZEN pre-rewrite
/// reference — not just the current direct lowering. Otherwise a
/// regression that moved both lowerings the same way would pass the
/// cross-lowering check.
#[test]
fn template_lowering_matches_the_frozen_reference() {
    let mut failures = Vec::new();
    for fixture in fixtures::all() {
        let template = fixture.template.expect("--features template");
        for mode in Mode::ALL {
            let recording = template(*mode);
            let result = std::panic::catch_unwind(|| {
                ui_lowering_parity::check_golden(fixture.name, *mode, &recording)
            });
            if result.is_err() {
                failures.push(format!("{}.{}", fixture.name, mode.suffix()));
            }
        }
    }
    assert!(failures.is_empty(), "template lowering differs from the frozen goldens for: {failures:?}");
}
