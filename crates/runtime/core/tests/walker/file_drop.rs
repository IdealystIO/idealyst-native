//! `on_file_drop` plumbing — assert that an OS file-drop handler attached to a
//! `view` survives mount, is installed on the backend via
//! `install_file_drop_handler`, and fires with the canonical `FileDropEvent`
//! shape when the backend synthesizes a drop. A plain `view` (no
//! `on_file_drop`) installs nothing.
//!
//! What's verified is the framework wiring (`Bound::on_file_drop` → walker →
//! `Backend::install_file_drop_handler`), not the per-backend drag listeners
//! (those live in each backend's own integration tests).

use std::cell::RefCell;
use std::rc::Rc;

use runtime_core::{
    view, DroppedFile, FileDropEvent, FileDropPhase, TouchPoint, TouchResponse,
};

use crate::common::{NodeId, TestRuntime};

/// A view WITHOUT `on_file_drop` installs no file-drop handler on the backend.
#[test]
fn plain_view_installs_no_file_drop_handler() {
    let rt = TestRuntime::new();
    let _owner = rt.render(view(Vec::new()).into());
    assert_eq!(
        rt.backend().file_drop_handler_count(),
        0,
        "a plain view must not install an OS file-drop handler"
    );
}

/// A view WITH `on_file_drop` installs exactly one handler, and the handler
/// fires with the delivered `FileDropEvent` when the backend synthesizes a drop.
#[test]
fn view_on_file_drop_installs_and_fires() {
    let rt = TestRuntime::new();

    let dropped: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let dropped_clone = dropped.clone();

    let _owner = rt.render(
        view(Vec::new())
            .on_file_drop(move |ev: &FileDropEvent| match &ev.phase {
                FileDropPhase::Dropped(files) => {
                    dropped_clone
                        .borrow_mut()
                        .extend(files.iter().map(|f| f.name.clone()));
                    TouchResponse::CONSUMED
                }
                FileDropPhase::Entered => TouchResponse::CONSUMED,
                _ => TouchResponse::IGNORED,
            })
            .into(),
    );

    // Mount-side: exactly one handler installed, on the (only) view node.
    assert_eq!(
        rt.backend().file_drop_handler_count(),
        1,
        "view().on_file_drop(..) must install exactly one handler"
    );

    // An `Entered` is accepted (CONSUMED) — this is what makes web/macOS accept
    // the drag.
    let entered = FileDropEvent {
        phase: FileDropPhase::Entered,
        position: TouchPoint::new(1.0, 2.0),
    };
    let resp = rt
        .backend()
        .fire_file_drop(NodeId(0), &entered)
        .expect("handler registered on the view node");
    assert!(resp.consumed, "Entered must be accepted so the drag isn't rejected");

    // A `Dropped` delivers the files to the handler.
    let drop = FileDropEvent {
        phase: FileDropPhase::Dropped(vec![DroppedFile {
            name: "photo.png".to_string(),
            mime: "image/png".to_string(),
            size: Some(1234),
            path: None,
            source: None,
        }]),
        position: TouchPoint::new(1.0, 2.0),
    };
    rt.backend()
        .fire_file_drop(NodeId(0), &drop)
        .expect("handler registered on the view node");
    assert_eq!(
        &*dropped.borrow(),
        &["photo.png".to_string()],
        "the dropped file's name must reach the handler"
    );
}
