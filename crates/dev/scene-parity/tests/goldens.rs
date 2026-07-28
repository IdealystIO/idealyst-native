//! One test per (scenario, mode) golden — plus a registry↔disk sync
//! check so a renamed scenario can't leave a stale golden behind (or a
//! registered mode silently miss its file).
//!
//! Regenerate all goldens: `UPDATE_GOLDENS=1 cargo test -p scene-parity`

use std::collections::BTreeSet;

use scene_parity::{check, golden_path, scenarios, Mode};

macro_rules! golden_test {
    ($test:ident, $name:literal, $mode:expr) => {
        #[test]
        fn $test() {
            check($name, $mode);
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

/// The goldens/ directory must contain EXACTLY one file per registered
/// (scenario, mode) pair — no strays from renamed/removed scenarios, no
/// registered pair missing its file.
#[test]
fn golden_files_match_registry_exactly() {
    let mut expected: BTreeSet<String> = BTreeSet::new();
    for s in scenarios() {
        for mode in s.modes {
            expected.insert(
                golden_path(s.name, *mode)
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }

    let dir = golden_path("_probe", Mode::Anchored)
        .parent()
        .unwrap()
        .to_path_buf();
    let actual: BTreeSet<String> = std::fs::read_dir(&dir)
        .unwrap_or_else(|_| panic!("goldens dir missing at {}", dir.display()))
        .filter_map(|e| {
            let name = e.ok()?.file_name().to_string_lossy().into_owned();
            name.ends_with(".golden").then_some(name)
        })
        .collect();

    assert_eq!(
        expected, actual,
        "goldens/ out of sync with the scenario registry \
         (left = registry, right = disk)",
    );
}
