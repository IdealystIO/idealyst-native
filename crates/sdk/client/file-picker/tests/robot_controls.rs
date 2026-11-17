//! A robot test hands the app files instead of driving the OS picker:
//! `file_picker.queue` answers the next pick with no dialog. A robot build:
//! `cargo test -p file-picker --features robot`.

#![cfg(feature = "robot")]

use std::future::Future;
use std::task::{Context, Poll, Waker};

fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = std::pin::pin!(fut);
    match fut.as_mut().poll(&mut Context::from_waker(Waker::noop())) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!("a queued answer resolves at once; this one would open the OS picker"),
    }
}

/// want_3321136e (CrewForge): uploading template PDFs, person documents
/// and certification files went through the OS picker, which no test can
/// drive. A queued answer is what the next pick returns, read like any
/// picked file; a queued cancel is a dismissed picker; each pick is logged.
#[test]
fn regression_a_queued_file_is_what_the_next_pick_returns_without_a_dialog() {
    use file_picker::{FilePicker, PickOutcome, PickRequest};
    use runtime_vocabulary::robot::bridge::invoke_command;
    use serde_json::json;

    let call = |name: &str, args: serde_json::Value| invoke_command("call_function", &json!({ "name": name, "args": args }));
    file_picker::register_robot_controls();
    call("file_picker.clear", json!({})).unwrap();

    // "%PDF-1.4" base64, and a text file.
    call(
        "file_picker.queue",
        json!({ "files": [
            { "name": "template.pdf", "base64": "JVBERi0xLjQ=" },
            { "name": "notes.txt", "text": "hello", "mime": "text/plain" },
        ] }),
    )
    .unwrap();
    call("file_picker.cancel_next", json!({})).unwrap();

    let PickOutcome::Picked(files) =
        block_on(FilePicker::new().pick(PickRequest::documents(["application/pdf"]).multiple())).unwrap()
    else {
        panic!("picked")
    };
    assert_eq!(files.len(), 2);
    assert_eq!((files[0].name(), files[0].mime(), files[0].size()), ("template.pdf", "application/pdf", Some(8)));
    assert_eq!(block_on(files[0].read_all()).unwrap(), b"%PDF-1.4");
    assert_eq!(block_on(files[1].read_all()).unwrap(), b"hello");
    assert!(files[0].path().is_none(), "in memory, read through open");

    assert!(matches!(block_on(FilePicker::new().pick(PickRequest::documents(Vec::<String>::new()))).unwrap(), PickOutcome::Cancelled));

    let log = call("file_picker.requests", json!({})).unwrap();
    assert!(log.contains("documents application/pdf multiple") && log.contains("\"documents\""), "{log}");

    let err = call("file_picker.queue", json!({ "files": [{ "name": "x", "base64": "%%%" }] })).unwrap_err();
    assert!(err.contains("base64"), "{err}");
    call("file_picker.clear", json!({})).unwrap();
}
