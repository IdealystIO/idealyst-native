//! FULL-OP golden tests, NEW core (the P2b gate): the SAME pairs against
//! the SAME `goldens_full/` files (modulo the README's sanctioned
//! divergences), plus registry-mirror and override-set sync checks.

use scene_parity::full::full_newcore_golden_path;
use scene_parity::full_new::{check_full_new, FULL_NEWCORE_OVERRIDES};
use scene_parity::scenarios_full::full_scenarios;
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

/// The new-core registry mirrors the old one exactly: same names, same
/// mode sets, same order.
#[test]
fn full_new_registry_matches_old() {
    let old = full_scenarios();
    let new = full_new_scenarios();
    assert_eq!(old.len(), new.len(), "scenario count mismatch");
    for (o, n) in old.iter().zip(new.iter()) {
        assert_eq!(o.name, n.name, "scenario name/order mismatch");
        assert_eq!(o.modes, n.modes, "mode set mismatch for `{}`", o.name);
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
