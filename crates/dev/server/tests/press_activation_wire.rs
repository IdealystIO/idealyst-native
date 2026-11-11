//! `pressable(..).activation(PressActivation::Down)` over the
//! runtime-server wire.

use std::rc::Rc;

use dev_server::WireRecordingBackend;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_vocabulary::caps::PressableOps;
use wire::Command;

#[test]
fn regression_press_down_pressable_crosses_the_wire_as_press_down() {
    // The recorder must tell the device which constructor to call: without
    // the flag every wire pressable mounted release-mode on the device.
    let mut recorder = WireRecordingBackend::new();
    let a11y = AccessibilityProps::default();
    PressableOps::create_pressable_on_down(&mut recorder, Rc::new(|| {}), &a11y);
    PressableOps::create_pressable(&mut recorder, Rc::new(|| {}), &a11y);
    let flags: Vec<bool> = recorder
        .drain_commands()
        .into_iter()
        .filter_map(|c| match c {
            Command::CreatePressable { on_down, .. } => Some(on_down),
            _ => None,
        })
        .collect();
    assert_eq!(flags, vec![true, false]);
}
