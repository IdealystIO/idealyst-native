//! Runnable example that demonstrates the recorder: realize a small
//! scene through [`dev_server::newcore::SceneSession`] against a
//! [`WireRecordingBackend`], and print the captured command stream as
//! pretty JSON.
//!
//! Run with:
//! ```text
//! cargo run -p dev-server --example dump_wire
//! ```

use dev_server::newcore::SceneSession;
use dev_server::WireRecordingBackend;
use runtime_vocabulary::builders::{button, text, view};

fn main() {
    let recorder = WireRecordingBackend::new();
    // Construct a small UI tree by hand. Real apps build this via the
    // `ui!` macro; the demo uses the vocabulary builders directly to
    // avoid pulling in macro infrastructure.
    let _session = SceneSession::mount(&recorder, |_registry| {}, || {
        view()
            .child(text().content("Hot reload demo"))
            .child(view().child(text().content("v0.1")))
            .child(
                button()
                    .label("Press me")
                    .on_press(|| println!("(dev) button fired — would mutate a signal")),
            )
            .build()
    });

    let commands = recorder.drain_commands();

    eprintln!(
        "# Wire dump — {} command(s) captured by WireRecordingBackend",
        commands.len()
    );
    eprintln!();
    let envelope = wire::DevToApp::Commands(commands);
    println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
}
