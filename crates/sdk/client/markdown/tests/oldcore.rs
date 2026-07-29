//! Old-core suite (default build): pins the `Element::External`
//! lowering of the authored surface (the byte-moved `oldcore` module)
//! so the dual-core restructure can't drift the default build. The web
//! handler itself is wasm32-gated; its DOM shape is pinned by
//! tests/newcore.rs, whose caps-generic handler is the same sequence
//! call-for-call.
#![cfg(not(feature = "new-core"))]

use std::any::TypeId;

use markdown::{markdown, MarkdownDoc, MdBlock, MdTheme};
use runtime_core::{Element, IntoElement};

/// `markdown(..)` lowers to `Element::External` keyed by
/// `MarkdownDoc`'s TypeId (backend handlers dispatch on it), carrying
/// the parsed doc + resolved theme as the payload, and starts childless.
#[test]
fn markdown_builds_an_external_keyed_by_markdown_doc() {
    let element = markdown("# Hi", MdTheme::light()).into_element();
    match element {
        Element::External {
            type_id,
            type_name,
            payload,
            children,
            ..
        } => {
            assert_eq!(type_id, TypeId::of::<MarkdownDoc>());
            assert!(type_name.contains("MarkdownDoc"));
            let doc = payload
                .downcast_ref::<MarkdownDoc>()
                .expect("payload is the parsed MarkdownDoc");
            assert_eq!(doc.blocks.len(), 1);
            assert!(matches!(doc.blocks[0], MdBlock::Heading { level: 1, .. }));
            assert_eq!(doc.theme, MdTheme::light());
            assert!(children.is_empty(), "markdown is a leaf");
        }
        _ => panic!("markdown must lower to Element::External"),
    }
}

/// Parse smoke via the public surface: inline styles reach the payload
/// runs (the author-side parse happens inside `markdown(..)`).
#[test]
fn parse_reaches_the_payload_through_the_public_surface() {
    let element = markdown("# Hello\n\nWorld **bold**", MdTheme::dark()).into_element();
    let Element::External { payload, .. } = element else {
        panic!("expected Element::External");
    };
    let doc = payload.downcast_ref::<MarkdownDoc>().expect("MarkdownDoc");
    assert_eq!(doc.blocks.len(), 2);
    let MdBlock::Paragraph { runs } = &doc.blocks[1] else {
        panic!("second block is a paragraph");
    };
    assert!(runs.iter().any(|r| r.bold && r.text == "bold"));
    assert_eq!(doc.theme, MdTheme::dark());
}

/// `markdown(..)` self-registers the (serialize, deserialize) wire pair
/// for `MarkdownDoc` — the recorder-side guarantee (codeblock pattern).
#[test]
fn wire_serde_roundtrips_the_doc() {
    let _ = markdown("- one\n- two", MdTheme::light());

    let name = std::any::type_name::<MarkdownDoc>();
    let doc = match markdown("- one\n- two", MdTheme::light()).into_element() {
        Element::External { payload, .. } => payload
            .downcast_ref::<MarkdownDoc>()
            .expect("MarkdownDoc")
            .clone(),
        _ => panic!("expected Element::External"),
    };
    let bytes = runtime_core::serialize_external_payload(name, &doc)
        .expect("serializer registered by markdown()");
    let back = runtime_core::deserialize_external_payload(name, &bytes)
        .expect("deserializer registered by markdown()");
    let back = back
        .downcast_ref::<MarkdownDoc>()
        .expect("roundtrip yields MarkdownDoc");
    assert_eq!(*back, doc);
}
