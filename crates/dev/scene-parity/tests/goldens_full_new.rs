//! FULL-OP golden tests: every pair against the frozen `goldens_full/`
//! files (modulo the README's sanctioned divergences), plus
//! frozen-registry and override-set sync checks.

use scene_parity::full::full_newcore_golden_path;
use scene_parity::full_new::{check_full_new, FULL_NEWCORE_OVERRIDES};
use scene_parity::scenarios_full_new::full_new_scenarios;
use scene_parity::Mode;

macro_rules! full_golden_new {
    ($test:ident, $name:literal, $mode:expr) => {
        #[test]
        fn $test() {
            check_full_new($name, $mode);
        }
    };
}

full_golden_new!(full_static_kitchen_sink_spliced, "full_static_kitchen_sink", Mode::Spliced);
full_golden_new!(full_reactive_text_spliced, "full_reactive_text", Mode::Spliced);
full_golden_new!(full_reactive_style_spliced, "full_reactive_style", Mode::Spliced);
full_golden_new!(full_prop_updates_spliced, "full_prop_updates", Mode::Spliced);
full_golden_new!(full_dyn_swap_primitives_anchored, "full_dyn_swap_primitives", Mode::Anchored);
full_golden_new!(full_dyn_swap_primitives_spliced, "full_dyn_swap_primitives", Mode::Spliced);
full_golden_new!(full_release_on_swap_anchored, "full_release_on_swap", Mode::Anchored);
full_golden_new!(full_release_on_swap_spliced, "full_release_on_swap", Mode::Spliced);
full_golden_new!(full_repeat_fallback_anchored, "full_repeat_fallback", Mode::Anchored);
full_golden_new!(full_repeat_fallback_spliced, "full_repeat_fallback", Mode::Spliced);
full_golden_new!(full_style_sheet_cohort_spliced, "full_style_sheet_cohort", Mode::Spliced);
full_golden_new!(full_style_state_overlay_spliced, "full_style_state_overlay", Mode::Spliced);
full_golden_new!(full_style_signal_class_spliced, "full_style_signal_class", Mode::Spliced);
full_golden_new!(full_style_preminted_spliced, "full_style_preminted", Mode::Spliced);
full_golden_new!(full_virtualizer_lifecycle_spliced, "full_virtualizer_lifecycle", Mode::Spliced);
full_golden_new!(full_virtualizer_lane_swap_anchored, "full_virtualizer_lane_swap", Mode::Anchored);
full_golden_new!(full_virtualizer_lane_swap_spliced, "full_virtualizer_lane_swap", Mode::Spliced);
full_golden_new!(full_graphics_lifecycle_anchored, "full_graphics_lifecycle", Mode::Anchored);
full_golden_new!(full_graphics_lifecycle_spliced, "full_graphics_lifecycle", Mode::Spliced);
full_golden_new!(full_portal_toggle_anchored, "full_portal_toggle", Mode::Anchored);
full_golden_new!(full_portal_toggle_spliced, "full_portal_toggle", Mode::Spliced);
full_golden_new!(full_overlay_static_spliced, "full_overlay_static", Mode::Spliced);
full_golden_new!(full_overlay_toggle_spliced, "full_overlay_toggle", Mode::Spliced);
full_golden_new!(full_presence_cycle_anchored, "full_presence_cycle", Mode::Anchored);
full_golden_new!(full_presence_cycle_spliced, "full_presence_cycle", Mode::Spliced);
full_golden_new!(full_presence_bare_spliced, "full_presence_bare", Mode::Spliced);
full_golden_new!(nav_swap_select_spliced, "nav_swap_select", Mode::Spliced);
full_golden_new!(nav_swap_dispose_evict_spliced, "nav_swap_dispose_evict", Mode::Spliced);
full_golden_new!(nav_stack_push_pop_spliced, "nav_stack_push_pop", Mode::Spliced);

/// The full-op scenario registry is FROZEN: this literal is the
/// (name, modes) inventory the old walker's `src/scenarios_full.rs`
/// registry carried when `goldens_full/` was generated. It replaces the
/// live `full_new_registry_matches_old` mirror, which needed the deleted
/// walker-side registry to compare against, and preserves the same
/// guarantee: no scenario silently disappears or loses a mode.
const FROZEN_FULL_SCENARIO_REGISTRY: &[(&str, &[Mode])] = &[
    ("full_static_kitchen_sink", &[Mode::Spliced]),
    ("full_reactive_text", &[Mode::Spliced]),
    ("full_reactive_style", &[Mode::Spliced]),
    ("full_prop_updates", &[Mode::Spliced]),
    ("full_dyn_swap_primitives", &[Mode::Anchored, Mode::Spliced]),
    ("full_release_on_swap", &[Mode::Anchored, Mode::Spliced]),
    ("full_repeat_fallback", &[Mode::Anchored, Mode::Spliced]),
    ("full_style_sheet_cohort", &[Mode::Spliced]),
    ("full_style_state_overlay", &[Mode::Spliced]),
    ("full_style_signal_class", &[Mode::Spliced]),
    ("full_style_preminted", &[Mode::Spliced]),
    ("full_virtualizer_lifecycle", &[Mode::Spliced]),
    ("full_virtualizer_lane_swap", &[Mode::Anchored, Mode::Spliced]),
    ("full_graphics_lifecycle", &[Mode::Anchored, Mode::Spliced]),
    ("full_portal_toggle", &[Mode::Anchored, Mode::Spliced]),
    ("full_overlay_static", &[Mode::Spliced]),
    ("full_overlay_toggle", &[Mode::Spliced]),
    ("full_presence_cycle", &[Mode::Anchored, Mode::Spliced]),
    ("full_presence_bare", &[Mode::Spliced]),
    ("nav_swap_select", &[Mode::Spliced]),
    ("nav_swap_dispose_evict", &[Mode::Spliced]),
    ("nav_stack_push_pop", &[Mode::Spliced]),
];

#[test]
fn full_scenario_registry_matches_the_frozen_inventory() {
    let live: Vec<(&str, &[Mode])> = full_new_scenarios()
        .iter()
        .map(|s| (s.name, s.modes))
        .collect();
    assert_eq!(
        FROZEN_FULL_SCENARIO_REGISTRY.len(),
        live.len(),
        "full-op scenario count drifted from the frozen inventory"
    );
    for (frozen, actual) in FROZEN_FULL_SCENARIO_REGISTRY.iter().zip(live.iter()) {
        assert_eq!(frozen.0, actual.0, "scenario name/order drift");
        assert_eq!(frozen.1, actual.1, "mode set drift for `{}`", frozen.0);
    }
}

/// Every sanctioned-override pair has its override golden on disk, and
/// every file in `goldens_full_newcore/` corresponds to a sanctioned
/// pair — the override set is closed.
#[test]
fn full_override_set_matches_disk() {
    let mut expected: Vec<String> = Vec::new();
    for &(name, mode) in FULL_NEWCORE_OVERRIDES {
        let path = full_newcore_golden_path(name, mode);
        assert!(
            path.exists() || std::env::var_os("UPDATE_NEWCORE_GOLDENS").is_some(),
            "missing override golden for ({name}, {mode:?}): {}",
            path.display()
        );
        expected.push(
            path.file_name()
                .expect("golden filename")
                .to_string_lossy()
                .into_owned(),
        );
    }
    let dir = full_newcore_golden_path("x", Mode::Spliced)
        .parent()
        .expect("goldens_full_newcore dir")
        .to_path_buf();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".golden") {
                assert!(
                    expected.contains(&name),
                    "orphan override golden {name} — not in FULL_NEWCORE_OVERRIDES"
                );
            }
        }
    }
}
