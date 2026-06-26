//! Reactive icon `data` — a live geometry source swaps the rendered glyph in
//! place (`Backend::update_icon_data`) without rebuilding the icon node.

#[path = "common/mock_backend.rs"]
mod mock_backend;
#[path = "common/runtime.rs"]
mod runtime;

use mock_backend::Event;
use runtime::TestRuntime;
use runtime_core::primitives::icon::IconData;
use runtime_core::{icon, signal, FillRule, IntoElement, Signal};

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
