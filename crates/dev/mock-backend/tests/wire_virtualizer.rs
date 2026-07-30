//! A scene containing a virtualizer, realized through the recorder's
//! `caps::VirtualizerOps::create_virtualizer`, must put
//! `Command::CreateVirtualizer` on the wire with the snapshotted
//! keys/sizes — not panic, and not silently drop the list.
//!
//! This test records WITHOUT replaying into a client: `dev-client`'s
//! `apply_create_virtualizer` is still the documented lazy-mount stub
//! (it registers no node, so replaying a virtualizer insert errors
//! `UnknownNode`) — a pre-existing gap, not a regression.

use runtime_shared::primitives::virtualizer::ItemSize;
use runtime_vocabulary::builders::{text, view, virtualizer};

#[test]
fn virtualizer_emits_create_virtualizer_with_snapshot() {
    dev_server::scheduler::install();
    let recorder = dev_server::WireRecordingBackend::new();

    let data = vec![
        (1u64, "Aria".to_string()),
        (2, "Bram".to_string()),
        (3, "Cleo".to_string()),
    ];
    let keys: Vec<u64> = data.iter().map(|(k, _)| *k).collect();
    let keys_for_scene = keys.clone();

    let _session = dev_server::newcore::SceneSession::mount(
        &recorder,
        |_r| {},
        move || {
            let rows = data.clone();
            let count = rows.len();
            view()
                .child(
                    virtualizer(
                        move || count,
                        move |i| keys_for_scene[i],
                        ItemSize::Known(std::rc::Rc::new(|_| 36.0)),
                        move |i| text().content(rows[i].1.clone()).build(),
                    )
                    .overscan(1.0),
                )
                .build()
        },
    );

    let create = recorder.drain_commands().into_iter().find_map(|c| match c {
        wire::Command::CreateVirtualizer { initial_keys, initial_size, layout, .. } => {
            Some((initial_keys, initial_size.sizes, layout))
        }
        _ => None,
    });
    let (got_keys, sizes, layout) =
        create.expect("no CreateVirtualizer command in the new-core recorded stream");
    assert_eq!(got_keys, keys, "initial keys should snapshot the data set");
    assert_eq!(sizes, vec![36.0, 36.0, 36.0], "initial sizes should snapshot ItemSize::Known");
    // Default layout crosses as a single-lane vertical list — the wire
    // mirror of `VirtualLayout::default()`.
    assert!(!layout.horizontal);
    assert_eq!(layout.lanes, wire::WireLanes::Fixed(1));
}
