//! `Element::External` payloads over the wire.
//!
//! An External (code block, table, maps, …) carries its data in a
//! type-erased `Rc<dyn Any>` payload. Before the fix, the recorder's
//! `create_external` dropped the payload and `Command::CreateExternal`
//! carried only `type_name` — so over the runtime-server wire the device
//! got a data-less node and rendered the "Component not available"
//! placeholder. (The drawer sidebar-adopt External worked only because
//! it's a pure sentinel carrying no data.)
//!
//! Now an SDK registers a serde pair via
//! `runtime_core::register_external_serde`; the recorder serializes the
//! payload into `CreateExternal { payload }`, and `dev-client`
//! deserializes it back to a concrete `Rc<dyn Any>` and dispatches to the
//! real `create_external` handler. `codeblock` registers its serde
//! lazily from `code_block(...)`, so this round-trips with no app-level
//! wiring.

use codeblock::{code_block, CodeBlockProps};
use mock_backend::WireHarness;
use runtime_core::Color;

#[test]
fn external_payload_round_trips_over_wire_to_real_handler() {
    let spans = vec![
        ("fn ".to_string(), Color("#888888".to_string())),
        ("hello".to_string(), Color("#00aa00".to_string())),
    ];
    let app_spans = spans.clone();
    let h = WireHarness::mount(move || code_block(app_spans.clone()).into());

    // The External's payload crossed the wire and the client dispatched it
    // to the real handler (recorded by the mock). Pre-fix this is `None` —
    // the recorder dropped the payload and the client placeholder'd it.
    let payload = {
        let scene = h.scene();
        scene
            .external_payload("CodeBlockProps")
            .cloned()
            .expect(
                "code block External must reach the client with a deserialized payload \
                 (pre-fix: payload dropped → not-available placeholder)",
            )
    };

    let props = payload
        .downcast::<CodeBlockProps>()
        .expect("reconstructed payload is a CodeBlockProps");

    // And the spans survived verbatim — text + color string per run.
    assert_eq!(props.spans.len(), 2, "both runs round-tripped");
    assert_eq!(props.spans[0].0, "fn ");
    assert_eq!(props.spans[0].1 .0, "#888888");
    assert_eq!(props.spans[1].0, "hello");
    assert_eq!(props.spans[1].1 .0, "#00aa00");
}
