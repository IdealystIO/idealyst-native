//! The P1 exit gate proper: every (scenario, mode) pair rerun against the
//! NEW scene core (`runtime-scene`) and asserted against the SAME golden
//! files the old walker pins — modulo only the README's sanctioned
//! divergences, handled explicitly by `new_core::normalize` (first-fire
//! anchor clears) and the closed `goldens_newcore/` override set
//! (cross-effect firing order). See `src/new_core.rs` for the mechanism.
//!
//! The old-core tests (`tests/goldens.rs`) are untouched and keep pinning
//! the walker byte-for-byte.

use std::collections::BTreeSet;

use scene_parity::new_core::{check_new, newcore_golden_path, NEWCORE_OVERRIDES};
use scene_parity::scenarios_new::new_scenarios;
use scene_parity::{scenarios, Mode};

macro_rules! golden_test {
    ($test:ident, $name:literal, $mode:expr) => {
        #[test]
        fn $test() {
            check_new($name, $mode);
        }
    };
}

// --- when ---
golden_test!(when_toggle_anchored, "when_toggle", Mode::Anchored);
golden_test!(when_toggle_spliced, "when_toggle", Mode::Spliced);
golden_test!(
    when_dedup_extra_signal_anchored,
    "when_dedup_extra_signal",
    Mode::Anchored
);
golden_test!(
    when_dedup_extra_signal_spliced,
    "when_dedup_extra_signal",
    Mode::Spliced
);

// --- switch ---
golden_test!(switch_rotation_anchored, "switch_rotation", Mode::Anchored);
golden_test!(switch_rotation_spliced, "switch_rotation", Mode::Spliced);

// --- each (keyed list) ---
golden_test!(each_append_anchored, "each_append", Mode::Anchored);
golden_test!(each_append_spliced, "each_append", Mode::Spliced);
golden_test!(each_remove_middle_spliced, "each_remove_middle", Mode::Spliced);
golden_test!(each_reverse_spliced, "each_reverse", Mode::Spliced);
golden_test!(
    each_insert_middle_survivors_spliced,
    "each_insert_middle_survivors",
    Mode::Spliced
);
golden_test!(
    each_multi_node_rows_spliced,
    "each_multi_node_rows",
    Mode::Spliced
);

// --- fragment index math ---
golden_test!(
    fragment_base_index_spliced,
    "fragment_base_index",
    Mode::Spliced
);

// --- dynamic ---
golden_test!(dynamic_swap_anchored, "dynamic_swap", Mode::Anchored);

// --- nesting ---
golden_test!(
    nested_when_in_each_row_anchored,
    "nested_when_in_each_row",
    Mode::Anchored
);
golden_test!(
    nested_when_in_each_row_spliced,
    "nested_when_in_each_row",
    Mode::Spliced
);

// --- dispose ordering ---
golden_test!(dispose_order_each_spliced, "dispose_order_each", Mode::Spliced);
golden_test!(
    dispose_order_when_anchored,
    "dispose_order_when",
    Mode::Anchored
);
golden_test!(dispose_order_when_spliced, "dispose_order_when", Mode::Spliced);

/// The new-core registry must mirror the old one exactly — same scenario
/// names, same modes — so both cores run the identical suite.
#[test]
fn new_core_registry_matches_old() {
    let old: BTreeSet<(String, Vec<String>)> = scenarios()
        .iter()
        .map(|s| {
            (
                s.name.to_string(),
                s.modes.iter().map(|m| m.suffix().to_string()).collect(),
            )
        })
        .collect();
    let new: BTreeSet<(String, Vec<String>)> = new_scenarios()
        .iter()
        .map(|s| {
            (
                s.name.to_string(),
                s.modes.iter().map(|m| m.suffix().to_string()).collect(),
            )
        })
        .collect();
    assert_eq!(old, new, "new-core scenario registry drifted from the old one");
}

/// `goldens_newcore/` may contain EXACTLY the sanctioned override files —
/// one per `NEWCORE_OVERRIDES` pair, nothing else. A stray file means an
/// unsanctioned divergence crept in.
#[test]
fn newcore_override_files_match_sanctioned_set() {
    let expected: BTreeSet<String> = NEWCORE_OVERRIDES
        .iter()
        .map(|(name, mode)| {
            newcore_golden_path(name, *mode)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    let dir = newcore_golden_path("_probe", Mode::Anchored)
        .parent()
        .unwrap()
        .to_path_buf();
    let actual: BTreeSet<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("goldens_newcore dir missing at {}", dir.display()))
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.ends_with(".golden").then_some(name)
        })
        .collect();

    assert_eq!(
        expected, actual,
        "goldens_newcore/ out of sync with NEWCORE_OVERRIDES \
         (left = sanctioned, right = disk)",
    );
}
