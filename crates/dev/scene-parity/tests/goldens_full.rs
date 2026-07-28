//! FULL-OP golden tests, OLD core (P2b): one test per (scenario, mode)
//! pair + a registry↔disk sync check. Pins the walker's complete
//! backend-call streams byte-for-byte.

use scene_parity::full::{check_full, full_golden_path};
use scene_parity::scenarios_full::full_scenarios;
use scene_parity::Mode;

macro_rules! full_golden {
    ($test:ident, $name:literal, $mode:expr) => {
        #[test]
        fn $test() {
            check_full($name, $mode);
        }
    };
}

full_golden!(full_static_kitchen_sink_spliced, "full_static_kitchen_sink", Mode::Spliced);
full_golden!(full_reactive_text_spliced, "full_reactive_text", Mode::Spliced);
full_golden!(full_reactive_style_spliced, "full_reactive_style", Mode::Spliced);
full_golden!(full_prop_updates_spliced, "full_prop_updates", Mode::Spliced);
full_golden!(full_dyn_swap_primitives_anchored, "full_dyn_swap_primitives", Mode::Anchored);
full_golden!(full_dyn_swap_primitives_spliced, "full_dyn_swap_primitives", Mode::Spliced);
full_golden!(full_release_on_swap_anchored, "full_release_on_swap", Mode::Anchored);
full_golden!(full_release_on_swap_spliced, "full_release_on_swap", Mode::Spliced);
full_golden!(full_repeat_fallback_anchored, "full_repeat_fallback", Mode::Anchored);
full_golden!(full_repeat_fallback_spliced, "full_repeat_fallback", Mode::Spliced);
full_golden!(full_style_sheet_cohort_spliced, "full_style_sheet_cohort", Mode::Spliced);
full_golden!(full_style_state_overlay_spliced, "full_style_state_overlay", Mode::Spliced);
full_golden!(full_style_signal_class_spliced, "full_style_signal_class", Mode::Spliced);
full_golden!(full_style_preminted_spliced, "full_style_preminted", Mode::Spliced);
full_golden!(full_virtualizer_lifecycle_spliced, "full_virtualizer_lifecycle", Mode::Spliced);
full_golden!(full_virtualizer_lane_swap_anchored, "full_virtualizer_lane_swap", Mode::Anchored);
full_golden!(full_virtualizer_lane_swap_spliced, "full_virtualizer_lane_swap", Mode::Spliced);
full_golden!(full_graphics_lifecycle_anchored, "full_graphics_lifecycle", Mode::Anchored);
full_golden!(full_graphics_lifecycle_spliced, "full_graphics_lifecycle", Mode::Spliced);
full_golden!(full_portal_toggle_anchored, "full_portal_toggle", Mode::Anchored);
full_golden!(full_portal_toggle_spliced, "full_portal_toggle", Mode::Spliced);
full_golden!(full_overlay_static_spliced, "full_overlay_static", Mode::Spliced);
full_golden!(full_overlay_toggle_spliced, "full_overlay_toggle", Mode::Spliced);
full_golden!(full_presence_cycle_anchored, "full_presence_cycle", Mode::Anchored);
full_golden!(full_presence_cycle_spliced, "full_presence_cycle", Mode::Spliced);
full_golden!(full_presence_bare_spliced, "full_presence_bare", Mode::Spliced);
full_golden!(nav_swap_select_spliced, "nav_swap_select", Mode::Spliced);
full_golden!(nav_swap_dispose_evict_spliced, "nav_swap_dispose_evict", Mode::Spliced);
full_golden!(nav_stack_push_pop_spliced, "nav_stack_push_pop", Mode::Spliced);

/// Every (scenario, mode) pair has a golden on disk, and every golden on
/// disk has a registered pair — no orphans in either direction.
#[test]
fn full_registry_matches_disk() {
    let mut expected: Vec<String> = Vec::new();
    for scenario in full_scenarios() {
        for &mode in scenario.modes {
            let path = full_golden_path(scenario.name, mode);
            assert!(
                path.exists() || std::env::var_os("UPDATE_GOLDENS").is_some(),
                "missing golden for ({}, {mode:?}): {}",
                scenario.name,
                path.display()
            );
            expected.push(
                path.file_name()
                    .expect("golden filename")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
    }
    let dir = full_golden_path("x", Mode::Spliced)
        .parent()
        .expect("goldens_full dir")
        .to_path_buf();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.ends_with(".golden") {
                assert!(
                    expected.contains(&name),
                    "orphan golden {name} in {} — no registered (scenario, mode) produces it",
                    dir.display()
                );
            }
        }
    }
}
