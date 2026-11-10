//! `text(selectable = true)` over the runtime-server wire.

use dev_server::WireRecordingBackend;
use runtime_shared::accessibility::AccessibilityProps;
use wire::Command;

#[test]
fn regression_selectable_text_crosses_the_wire_as_selectable() {
    // `CreateText` carried no selectable flag, so the device always
    // created an unselectable text.
    let mut recorder = WireRecordingBackend::new();
    let a11y = AccessibilityProps::default();
    recorder.create_selectable_text("copy me", &a11y);
    recorder.create_text("plain", &a11y);
    let flags: Vec<(String, bool)> = recorder
        .drain_commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::CreateText { content, selectable, .. } => Some((content, selectable)),
            _ => None,
        })
        .collect();
    assert_eq!(flags, vec![("copy me".to_string(), true), ("plain".to_string(), false)]);
}
