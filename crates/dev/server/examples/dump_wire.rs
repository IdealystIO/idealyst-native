//! Runnable example that demonstrates the prototype: build a small
//! Primitive tree, drive it through `framework_core::render(...)`
//! against a [`WireRecordingBackend`], and print the captured
//! command stream as pretty JSON.
//!
//! Run with:
//! ```text
//! cargo run -p dev-server --example dump_wire
//! ```

use std::cell::RefCell;
use std::rc::Rc;

use framework_core::{render, IntoAction, Primitive, TextSource};
use dev_server::WireRecordingBackend;

fn main() {
    // Construct a small UI tree by hand. Real apps build this via
    // the `ui!` macro; for the demo we keep it raw to avoid pulling
    // in macro infrastructure.
    let tree = Primitive::View {
        children: vec![
            Primitive::Text {
                source: TextSource::Static("Hot reload demo".into()),
                style: None,
                ref_fill: None,
                accessibility: Default::default(),
                test_id: None,
            },
            Primitive::View {
                children: vec![Primitive::Text {
                    source: TextSource::Static("v0.1".into()),
                    style: None,
                    ref_fill: None,
                    accessibility: Default::default(),
                    test_id: None,
                }],
                style: None,
                ref_fill: None,
                safe_area_sides: framework_core::SafeAreaSides::NONE,
                on_touch: None,
                accessibility: Default::default(),
                test_id: None,
            },
            Primitive::Button {
                label: TextSource::Static("Press me".into()),
                on_click: (|| {
                    println!("(dev) button fired — would mutate a signal");
                })
                .into_action(),
                leading_icon: None,
                trailing_icon: None,
                style: None,
                ref_fill: None,
                disabled: None,
                accessibility: Default::default(),
                test_id: None,
            },
        ],
        style: None,
        ref_fill: None,
        safe_area_sides: framework_core::SafeAreaSides::NONE,
        on_touch: None,
        accessibility: Default::default(),
        test_id: None,
    };

    let recorder = WireRecordingBackend::new();
    let backend_rc = Rc::new(RefCell::new(recorder.clone()));
    let _owner = render(backend_rc, tree);

    let commands = recorder.drain_commands();

    eprintln!(
        "# Wire dump — {} command(s) captured by WireRecordingBackend",
        commands.len()
    );
    eprintln!();
    let envelope = wire::DevToApp::Commands(commands);
    println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
}
