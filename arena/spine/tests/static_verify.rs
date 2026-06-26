//! End-to-end check of the static tier: load the real `todo-app` scenario +
//! rubric, then verify every static-tier decision item against a fixture
//! project that satisfies them. This exercises rubric loading/validation, the
//! scenario↔rubric id link, and the StaticAstVerifier against real files —
//! without needing a compiler or a live agent.

use arena_spine::rubric::{Rubric, Tier};
use arena_spine::verify::static_ast::StaticAstVerifier;
use arena_spine::verify::{RunContext, Verifier};
use arena_spine::Scenario;
use std::path::PathBuf;

fn scenario_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR == arena/spine; scenarios live at arena/scenarios.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("scenarios")
        .join("todo-app")
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("todo-app-sample")
}

#[test]
fn todo_app_scenario_and_rubric_load_and_link() {
    let scenario = Scenario::load(&scenario_dir().join("scenario.toml")).expect("scenario loads");
    let rubric = Rubric::load(&scenario_dir().join("rubric.toml")).expect("rubric loads + validates");
    assert_eq!(scenario.id, rubric.scenario_id, "ids must match");
    assert!(!rubric.items.is_empty());
}

#[test]
fn every_static_decision_item_passes_against_the_fixture() {
    let rubric = Rubric::load(&scenario_dir().join("rubric.toml")).expect("rubric loads");
    let ctx = RunContext::source_only(fixture_dir());
    let verifier = StaticAstVerifier;

    let static_items: Vec<_> = rubric
        .items
        .iter()
        .filter(|i| i.tier == Tier::Static)
        .collect();
    assert!(
        !static_items.is_empty(),
        "the todo-app rubric should have static items"
    );

    for item in static_items {
        let result = verifier.verify(item, &ctx);
        assert!(
            result.passed,
            "static item `{}` should pass against the fixture but failed: {}",
            item.id, result.evidence
        );
    }
}

#[test]
fn static_verifier_fails_a_pattern_the_fixture_lacks() {
    // Guards against a verifier that passes everything: a pattern the fixture
    // genuinely does not contain must fail.
    let rubric = Rubric::load(&scenario_dir().join("rubric.toml")).expect("rubric loads");
    let mut item = rubric
        .items
        .iter()
        .find(|i| i.tier == Tier::Static)
        .cloned()
        .expect("a static item exists");
    item.assertion.pattern = Some("this_symbol_does_not_exist_anywhere_xyzzy".into());
    item.assertion.min_count = None;

    let ctx = RunContext::source_only(fixture_dir());
    let result = StaticAstVerifier.verify(&item, &ctx);
    assert!(!result.passed, "absent pattern must fail: {}", result.evidence);
}
