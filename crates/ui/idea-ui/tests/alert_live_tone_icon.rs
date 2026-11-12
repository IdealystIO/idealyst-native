//! An `Alert` with a live `tone` and the default `AlertIcon::Auto` swaps its
//! leading glyph when the tone changes, mounted through the real `realize`
//! path against `host-mock`. The icon's `data` is live, so the swap reaches
//! the backend as `update_icon_data` with the new paths; a build-time
//! snapshot of the tone would leave an info circle on an alert that turned
//! into a success.

use idea_ui::{install_idea_theme, light_theme, tone, Alert, ToneRef};
use runtime_core::{signal, ui};

/// Lucide `circle-check`'s check stroke — only the Success glyph has it.
const CHECK_STROKE: &str = "m9 12 2 2 4-4";

#[test]
fn a_live_tone_swaps_the_auto_icon() {
    let harness = host_mock::Harness::new();
    harness.record_all();
    let (tree, live) = harness.world.enter(|| {
        install_idea_theme(light_theme());
        let live = signal(ToneRef::from(tone::Info));
        let tree = ui! { Alert(title = "Syncing".to_string(), tone = live) };
        (tree, live)
    });
    let _realized = harness.mount(tree);
    harness.flush();
    let before = harness.ops().join("\n");
    assert!(before.lines().any(|l| l.starts_with("create ") && l.ends_with(" icon")), "an Auto alert draws a glyph:\n{before}");
    assert!(!before.contains(CHECK_STROKE));

    harness.world.enter(|| live.set(ToneRef::from(tone::Success)));
    harness.flush();
    let after = harness.ops().join("\n");
    let swap = harness.ops().into_iter().rev().find(|o| o.starts_with("update_icon_data"));
    assert!(
        swap.as_deref().is_some_and(|o| o.contains(CHECK_STROKE)),
        "the glyph swapped to the success check:\n{after}"
    );
}
