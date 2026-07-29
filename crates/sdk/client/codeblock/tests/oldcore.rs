//! Old-core suite (default build): the `Element::External` surface —
//! payload shape and the wire serde `code_block` self-registers — on
//! the host triple (the web handler itself is wasm32-gated; its DOM
//! shape is pinned by tests/newcore.rs, whose handler is the same
//! sequence call-for-call).
#![cfg(not(feature = "new-core"))]

use codeblock::{code_block, CodeBlockProps};
use runtime_core::{Color, Element, IntoElement};

fn spans() -> Vec<(String, Color)> {
    vec![
        ("fn ".into(), Color("#888".into())),
        ("hello".into(), Color("#0a0".into())),
    ]
}

#[test]
fn code_block_builds_an_external_with_the_span_payload() {
    let element = code_block(spans()).into_element();
    match element {
        Element::External {
            type_id,
            payload,
            children,
            ..
        } => {
            assert_eq!(type_id, std::any::TypeId::of::<CodeBlockProps>());
            let props = payload
                .downcast_ref::<CodeBlockProps>()
                .expect("payload is CodeBlockProps");
            assert_eq!(props.spans, spans());
            assert!(children.is_empty(), "code_block is a leaf");
        }
        _ => panic!("code_block must lower to Element::External"),
    }
}

#[test]
fn wire_serde_roundtrips_the_spans() {
    // `code_block` self-registers the (serialize, deserialize) pair —
    // the recorder-side guarantee.
    let _ = code_block(spans());

    let name = std::any::type_name::<CodeBlockProps>();
    let props = CodeBlockProps { spans: spans() };
    let bytes = runtime_core::serialize_external_payload(name, &props)
        .expect("serializer registered by code_block()");
    let back = runtime_core::deserialize_external_payload(name, &bytes)
        .expect("deserializer registered by code_block()");
    let back = back
        .downcast_ref::<CodeBlockProps>()
        .expect("roundtrip yields CodeBlockProps");
    assert_eq!(back.spans, spans());
}
