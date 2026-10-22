//! Style parity end to end: a scene realized through the dev-server
//! recorder, encoded over `wire::codec`, replayed by `dev-client` into
//! the mock backend, must style its nodes with the same rules the author
//! wrote — the properties `--local` would hand the backend directly.
//!
//! The two properties here are the ones a user reported missing in
//! runtime-server mode: a button's `align_self: FlexStart` (it stretched
//! full width instead) and a label's `text_transform: Uppercase` (it
//! rendered as typed). The exhaustive per-property round trip lives in
//! `wire/tests/style_parity.rs`; this one proves the whole recorder →
//! codec → replay path delivers them to a backend.

use mock_backend::WireHarness;
use runtime_shared::style::{AlignSelf, TextTransform};
use runtime_shared::StyleRules;
use runtime_vocabulary::builders::{button, text, view};

#[test]
fn regression_align_self_and_text_transform_reach_the_client_backend() {
    let harness = WireHarness::mount(|| {
        view()
            .child(
                button()
                    .label("Save")
                    .on_press(|| {})
                    .style(StyleRules {
                        align_self: Some(AlignSelf::FlexStart),
                        ..StyleRules::default()
                    }),
            )
            .child(text().content("shouting").style(StyleRules {
                text_transform: Some(TextTransform::Uppercase),
                ..StyleRules::default()
            }))
            .build()
    });
    let scene = harness.scene();

    let style_of = |needle: &str| {
        let id = scene.find_by_text(needle).unwrap_or_else(|| panic!("{needle} replayed"));
        scene
            .node(id)
            .and_then(|n| n.last_style.clone())
            .unwrap_or_else(|| panic!("{needle}'s style crossed the wire:\n{}", scene.dump()))
    };

    assert_eq!(
        style_of("Save").align_self,
        Some(AlignSelf::FlexStart),
        "a button's align_self must reach the client, or it stretches full width",
    );
    assert_eq!(
        style_of("shouting").text_transform,
        Some(TextTransform::Uppercase),
        "a text's text_transform must reach the client",
    );
}
