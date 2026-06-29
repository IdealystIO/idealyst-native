//! Reactive icon `data` — a live geometry source swaps the rendered glyph in
//! place (`Backend::update_icon_data`) without rebuilding the icon node.

#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use mock_backend::Event;
use runtime::TestRuntime;
use runtime_core::primitives::icon::IconData;
use runtime_core::{arena_stats, icon, signal, FillRule, IntoElement, Signal};

const GLYPH_A: IconData = IconData {
    view_box: (24, 24),
    paths: &["M1 1"],
    fill_rule: FillRule::NonZero,
    filled: false,
};
const GLYPH_B: IconData = IconData {
    view_box: (24, 24),
    paths: &["M9 9"],
    fill_rule: FillRule::NonZero,
    filled: false,
};

#[test]
fn reactive_icon_data_swaps_glyph_in_place_without_rebuild() {
    let rt = TestRuntime::new();
    let toggled: Signal<bool> = signal!(false);

    let tree = icon(GLYPH_A)
        .data(move || if toggled.get() { GLYPH_B } else { GLYPH_A })
        .into_element();
    let _owner = rt.render(tree);

    rt.backend_mut().clear_events();
    toggled.set(true);
    let evs = rt.events();
    assert!(
        evs.iter().any(|e| matches!(
            e,
            Event::UpdateIconData { paths, .. } if paths == &vec!["M9 9".to_string()]
        )),
        "flipping the data source must push update_icon_data with the new glyph: {:?}",
        evs
    );
    assert!(
        !evs.iter().any(|e| matches!(e, Event::CreateIcon)),
        "the icon node must NOT be rebuilt: {:?}",
        evs
    );
}

/// Teardown: the scope-owned `Effect::new(...)` installed by `icon().data(..)`
/// in the walker (`walker/icon.rs`) must be FREED when the render `Owner` drops.
/// After unmount, flipping the source must NOT reach the (now-released) backend
/// — no further `update_icon_data` — and the arena's `effects_in_use` must
/// return to its pre-render baseline.
#[test]
fn reactive_icon_data_effect_is_freed_on_owner_drop() {
    let rt = TestRuntime::new();
    let toggled: Signal<bool> = signal!(false);

    let effects_baseline = arena_stats().effects_in_use;

    let tree = icon(GLYPH_A)
        .data(move || if toggled.get() { GLYPH_B } else { GLYPH_A })
        .into_element();
    let owner = rt.render(tree);

    // Sanity: while mounted, flipping the source fires the in-place update.
    rt.backend_mut().clear_events();
    toggled.set(true);
    assert!(
        rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateIconData { .. })),
        "while mounted, flipping the source must fire update_icon_data: {:?}",
        rt.events()
    );

    // The data effect occupies an arena slot while the owner is alive.
    assert!(
        arena_stats().effects_in_use > effects_baseline,
        "the reactive data effect should occupy an arena slot while mounted",
    );

    // Drop the render scope: the scope-owned effect must be released with it.
    drop(owner);

    // Effect count returns toward baseline (assertion b).
    assert_eq!(
        arena_stats().effects_in_use,
        effects_baseline,
        "after Owner drop, the data effect must be freed (effects_in_use back to baseline)",
    );

    // Behavioral proof (assertion a): flipping the source again must NOT reach
    // the freed backend effect — no further update_icon_data.
    rt.backend_mut().clear_events();
    toggled.set(false);
    assert!(
        !rt.events()
            .iter()
            .any(|e| matches!(e, Event::UpdateIconData { .. })),
        "after Owner drop, flipping the source must NOT fire update_icon_data \
         (the effect was freed with the scope): {:?}",
        rt.events()
    );
}
