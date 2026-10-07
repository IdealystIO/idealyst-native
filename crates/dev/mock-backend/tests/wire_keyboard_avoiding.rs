//! `keyboard_avoiding_view` over the wire. Only the opt-in crosses: the
//! recorder is headless (no keyboard), so the CLIENT backend observes its
//! own soft keyboard and animates the view. Without the relay a runtime-server
//! dev session had no keyboard avoidance at all, while `--local` did.

use mock_backend::WireHarness;
use runtime_shared::{KeyboardAvoid, KeyboardAvoidBehavior};
use runtime_vocabulary::builders::{keyboard_avoiding_view, text};

#[test]
fn regression_keyboard_avoiding_opt_in_crosses_the_wire() {
    let h = WireHarness::mount(|| {
        keyboard_avoiding_view()
            .behavior(KeyboardAvoidBehavior::Translate)
            .animated(false)
            .child(text().content("FORM"))
            .build()
    });
    assert_eq!(
        h.scene().keyboard_avoiding(),
        Some(KeyboardAvoid { behavior: KeyboardAvoidBehavior::Translate, animated: false }),
        "the client backend must receive the opt-in verbatim"
    );
}

/// A client that connects after the mount gets the opt-in in its catch-up
/// snapshot.
#[test]
fn keyboard_avoiding_opt_in_survives_in_the_late_joiner_snapshot() {
    let h = WireHarness::mount(|| {
        keyboard_avoiding_view().child(text().content("CHAT")).build()
    });
    let snapshot = h.snapshot();
    assert!(
        snapshot.iter().any(|c| matches!(
            c,
            wire::Command::MarkKeyboardAvoiding { behavior: 0, animated: true, .. }
        )),
        "snapshot must carry the opt-in; got {snapshot:#?}"
    );
}
