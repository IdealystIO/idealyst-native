//! Third-party primitive payloads over the wire.
//!
//! There are two mechanisms, and this file pins both.
//!
//! **1. The normal path — the SDK's portable handler runs on the dev
//! side.** A third-party primitive registers a caps-generic mount
//! handler on the scene registry (`codeblock::register`), and the
//! runtime-server recorder is caps-complete, so that handler runs
//! against the RECORDER. Its output is ordinary node ops
//! (`CreateElement`/`CreateText`/`ApplyStyle`/`Insert`), which every
//! replay client already understands. Nothing type-erased crosses at
//! all: the payload never leaves the dev process. This is what
//! `Element::External` used to need a bespoke wire path for.
//!
//! **2. The escape hatch — `Command::CreateExternal` + a payload
//! serde.** A handler that cannot run on the recorder (a platform-only
//! widget) still has to ship its data to the device. `wire::payload_serde`
//! is the registry for that: the SDK registers a
//! (serialize, deserialize) pair keyed by `type_name`, the recorder's
//! `caps::ExternalOps::create_external` serializes into
//! `Command::CreateExternal { payload }`, and `dev-client` decodes it
//! back to a concrete `Rc<dyn Any>` and dispatches to the real
//! `create_external` handler. Pre-fix, the payload was dropped and the
//! device rendered "Component not available".

use std::any::Any;
use std::rc::Rc;

use mock_backend::WireHarness;
use runtime_shared::accessibility::AccessibilityProps;
use runtime_shared::Color;
use runtime_vocabulary::caps::ExternalOps;

// --- 1. Portable handler on the recorder -----------------------------------

#[test]
fn third_party_handler_runs_on_the_recorder_and_ships_generic_node_ops() {
    let spans = vec![
        ("fn ".to_string(), Color("#888888".to_string())),
        ("hello".to_string(), Color("#00aa00".to_string())),
    ];
    let app_spans = spans.clone();

    let h = WireHarness::mount_with(codeblock::register, move || {
        runtime_vocabulary::glue::IntoElement::into_element(codeblock::code_block(app_spans.clone()))
    });

    let scene = h.scene();
    // The spans reconstructed as real text nodes on the client — i.e.
    // the SDK's content crossed, without a payload serde and without a
    // primitive-specific wire command.
    assert!(
        scene.contains_text("fn "),
        "code-block span content must reach the client:\n{}",
        scene.dump()
    );
    assert!(
        scene.contains_text("hello"),
        "code-block span content must reach the client:\n{}",
        scene.dump()
    );
    assert!(
        scene.external_payload("CodeBlock").is_none(),
        "a portable handler must NOT need the CreateExternal escape hatch",
    );
}

// --- 2. The `CreateExternal` payload escape hatch ---------------------------

/// A payload type standing in for a platform-only widget's props: the
/// recorder has no handler that can build it, so its data has to cross
/// as bytes.
#[derive(Debug, PartialEq)]
struct NativeWidgetProps {
    caption: String,
}

const NATIVE_WIDGET: &str = "NativeWidgetProps";

fn register_serde() {
    wire::register_external_serde(
        NATIVE_WIDGET,
        |any| {
            any.downcast_ref::<NativeWidgetProps>()
                .map(|p| p.caption.as_bytes().to_vec())
        },
        |bytes| {
            Some(Rc::new(NativeWidgetProps {
                caption: String::from_utf8(bytes.to_vec()).ok()?,
            }) as Rc<dyn Any>)
        },
    );
}

#[test]
fn external_payload_round_trips_over_wire_to_the_real_handler() {
    register_serde();

    // Drive the recorder's `create_external` directly: on the surviving
    // core this cap is reached from a handler that chose the escape
    // hatch (or from `missing_primitive_placeholder`), never from a
    // walker, so the cap IS the seam under test.
    let recorder = dev_server::WireRecordingBackend::new();
    {
        let mut rec = recorder.clone();
        let payload: Rc<dyn Any> = Rc::new(NativeWidgetProps {
            caption: "hello from the dev side".into(),
        });
        ExternalOps::create_external(
            &mut rec,
            std::any::TypeId::of::<NativeWidgetProps>(),
            NATIVE_WIDGET,
            &payload,
            &AccessibilityProps::default(),
        );
    }

    let cmds = recorder.drain_commands();
    let carried = cmds
        .iter()
        .find_map(|c| match c {
            wire::Command::CreateExternal { type_name, payload, .. } => {
                Some((type_name.clone(), payload.clone()))
            }
            _ => None,
        })
        .expect("the recorder must emit CreateExternal");
    assert_eq!(carried.0, NATIVE_WIDGET);
    assert!(
        !carried.1.is_empty(),
        "the registered serde must put the payload on the wire (pre-fix: dropped)"
    );

    // Client half: replay into a MockBackend and prove the deserialized
    // payload reached the real `create_external` handler.
    let (tx, _rx) = std::sync::mpsc::channel();
    let mut client = dev_client::WireBackend::new(mock_backend::MockBackend::new(), tx);
    let bytes = wire::codec::encode(&wire::DevToApp::Commands(cmds)).expect("encode");
    match wire::codec::decode::<wire::DevToApp>(&bytes).expect("decode") {
        wire::DevToApp::Commands(c) => client.apply_batch(c).expect("replay"),
        other => panic!("expected Commands, got {other:?}"),
    };

    let backend = client.backend().borrow();
    let payload = backend
        .external_payload(NATIVE_WIDGET)
        .cloned()
        .expect("the payload must reach the client's real handler, not the placeholder");
    let props = payload
        .downcast::<NativeWidgetProps>()
        .expect("reconstructed payload is a NativeWidgetProps");
    assert_eq!(props.caption, "hello from the dev side");
}

#[test]
fn external_without_a_registered_serde_reaches_the_client_as_a_placeholder() {
    let recorder = dev_server::WireRecordingBackend::new();
    {
        let mut rec = recorder.clone();
        // `missing_primitive_placeholder` is the in-tree caller of this
        // shape: a sentinel payload with no serde.
        ExternalOps::missing_primitive_placeholder(&mut rec, "SomeUnbuiltPrimitive");
    }
    let cmds = recorder.drain_commands();
    let payload = cmds
        .iter()
        .find_map(|c| match c {
            wire::Command::CreateExternal { payload, .. } => Some(payload.clone()),
            _ => None,
        })
        .expect("CreateExternal emitted");
    assert!(
        payload.is_empty(),
        "a payload with no registered serde must cross empty, so the client \
         renders the not-available placeholder rather than a wrong widget"
    );
}
