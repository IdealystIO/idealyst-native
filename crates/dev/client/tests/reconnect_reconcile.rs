//! Reconnect reconciliation: when a persisted `WireBackend` re-applies a
//! snapshot whose `Create*` carries reactive state folded in by the recorder
//! (see `dev/server/src/scene_model.rs`), the already-held node must be
//! UPDATED to the folded values — not early-returned, which would leave it
//! stale. Regression test for the reactive-by-default `Update*` variants
//! (secure/placeholder/value, image src/alt, …).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::mpsc;

use dev_client::WireBackend;
use mock_backend::MockBackend;
use wire::{AppToDev, Command, HandlerId, NodeId};

#[test]
fn reconnect_reapplies_folded_reactive_fields() {
    let (tx, _rx) = mpsc::channel::<AppToDev>();
    let backend = Rc::new(RefCell::new(MockBackend::new()));
    let mut wb = WireBackend::new_with_shared(backend.clone(), tx);

    // Initial mount: an unmasked text input ("old") + an image ("a.png").
    // The mock mints ids 1, 2 in create order.
    wb.apply(Command::CreateTextInput {
        id: NodeId(1),
        initial_value: "old".into(),
        placeholder: None,
        on_change: HandlerId(0),
        secure: false,
        a11y: Default::default(),
    })
    .unwrap();
    wb.apply(Command::CreateImage {
        id: NodeId(2),
        src: "a.png".into(),
        alt: None,
        a11y: Default::default(),
    })
    .unwrap();

    // A reconnect snapshot re-emits the SAME node ids with the recorder's
    // folded reactive state (secure flipped on, value + src changed). Before
    // the fix the `Create*` arms early-returned and dropped all of this.
    wb.apply(Command::CreateTextInput {
        id: NodeId(1),
        initial_value: "new".into(),
        placeholder: None,
        on_change: HandlerId(0),
        secure: true,
        a11y: Default::default(),
    })
    .unwrap();
    wb.apply(Command::CreateImage {
        id: NodeId(2),
        src: "b.png".into(),
        alt: None,
        a11y: Default::default(),
    })
    .unwrap();

    let b = backend.borrow();
    let ti = b.node(1).expect("text input node (mock id 1)");
    assert!(
        ti.secure,
        "reconnect must re-apply the folded `secure` flag, not early-return",
    );
    assert_eq!(
        ti.text.as_deref(),
        Some("new"),
        "reconnect must re-apply the folded text-input value",
    );
    let img = b.node(2).expect("image node (mock id 2)");
    assert_eq!(
        img.image_src.as_deref(),
        Some("b.png"),
        "reconnect must re-apply the folded image src",
    );
}
