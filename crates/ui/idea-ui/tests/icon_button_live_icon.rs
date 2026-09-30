//! An `IconButton`'s vector icon follows the app live, mounted through
//! the real `realize` path against `host-mock`:
//!
//! - a live `icon` swaps the glyph in place (a light/dark toggle's
//!   sun ↔ moon). It used to be snapshotted at build.
//! - the icon's tint follows a theme swap. It's stamped on the icon's own
//!   node as `.color(move || fg.resolve())`, and that effect never re-ran
//!   on the new kernel, because `resolve()` read a legacy-arena token
//!   signal no world effect subscribes to. After a swap to dark, the
//!   Inspector's icons kept the light theme's dark gray.

use std::rc::Rc;

use idea_ui::{dark_theme, install_idea_theme, light_theme, set_idea_theme, tone, variant, IconButton};
use runtime_core::{signal, ui, FillRule, IconData};

const SUN: IconData = IconData { view_box: (24, 24), paths: &["M12 2v2"], fill_rule: FillRule::NonZero, filled: false };
const MOON: IconData = IconData { view_box: (24, 24), paths: &["M12 3a6 6 0 0 0 9 9"], fill_rule: FillRule::NonZero, filled: false };

fn icon_color_ops(harness: &host_mock::Harness) -> Vec<String> {
    harness.ops().into_iter().filter(|o| o.starts_with("update_icon_color")).collect()
}

#[test]
fn a_live_icon_swaps_in_place() {
    let harness = host_mock::Harness::new();
    harness.record_all();
    let (tree, dark) = harness.world.enter(|| {
        install_idea_theme(light_theme());
        let dark = signal(false);
        let icon = runtime_core::memo(move || Some(if dark.get() { SUN } else { MOON }));
        let tree = ui! {
            IconButton(
                glyph = String::new(),
                icon = icon,
                on_click = Rc::new(|| {}) as Rc<dyn Fn()>,
            )
        };
        (tree, dark)
    });
    let _realized = harness.mount(tree);
    harness.flush();

    harness.world.enter(|| dark.set(true));
    harness.flush();
    let ops = harness.ops().join("\n");
    assert!(ops.contains(&format!("update_icon_data n")) && ops.contains("M12 2v2"), "the glyph swapped to SUN:\n{ops}");
}

#[test]
fn regression_the_icon_tint_follows_a_theme_swap() {
    let harness = host_mock::Harness::new();
    harness.record_all();
    let tree = harness.world.enter(|| {
        install_idea_theme(light_theme());
        ui! {
            IconButton(
                glyph = String::new(),
                icon = Some(SUN),
                on_click = Rc::new(|| {}) as Rc<dyn Fn()>,
                tone = tone::Neutral,
                variant = variant::Ghost,
            )
        }
    });
    let _realized = harness.mount(tree);
    harness.flush();
    let light = icon_color_ops(&harness);
    assert!(!light.is_empty(), "the icon is tinted:\n{}", harness.ops().join("\n"));

    harness.world.enter(|| set_idea_theme(dark_theme()));
    harness.flush();
    let after = icon_color_ops(&harness);
    assert!(after.len() > light.len(), "the swap re-tinted the icon:\n{}", harness.ops().join("\n"));
    assert_ne!(after.last(), light.last(), "with the dark theme's color");
}
