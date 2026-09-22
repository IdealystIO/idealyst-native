//! The dev server must APPLY an overlay patch, not forward it.
//!
//! In runtime-server mode the app runs in the sidecar, and every backend
//! call the sidecar makes funnels through the recorder's single emit
//! point — which is what keeps its scene MIRROR current. The mirror is
//! what a client connecting mid-session is snapshotted from.
//!
//! So applying a patch locally updates both the attached clients (the
//! calls land in the broadcast log) and the ones that have not connected
//! yet (the same calls update the mirror). Forwarding
//! `DevToApp::OverlayPatch` to the clients instead would do only the
//! first, and the next client to connect would snapshot the OLD text —
//! a bug that shows up as "it worked until I opened a second tab".
//!
//! This test is the guard on that arrangement: it patches a mounted
//! scene and then asserts on the SNAPSHOT, which is the half that
//! forwarding would break.

#![cfg(feature = "ui-overlay")]

use std::borrow::Cow;

use dev_server::WireRecordingBackend;
use runtime_macros::ui;
use runtime_template::{Edit, LiteralValue};
use runtime_vocabulary::glue::Element;
use wire::Command;

fn app() -> Element {
    ui! {
        view() {
            text { "before" }
        }
    }
}

/// Every text a snapshot would create, in order.
fn snapshot_texts(recorder: &WireRecordingBackend) -> Vec<String> {
    recorder
        .snapshot()
        .into_iter()
        .filter_map(|c| match c {
            Command::CreateText { content, .. } => Some(content),
            _ => None,
        })
        .collect()
}

#[test]
fn a_patch_applied_by_the_server_reaches_a_late_joining_client() {
    runtime_vocabulary::overlay::reset();
    let recorder = WireRecordingBackend::new();
    let mut session = dev_server::newcore::SceneSession::mount(&recorder, |_r| {}, app);

    assert_eq!(snapshot_texts(&recorder), vec!["before".to_string()]);

    // The site key comes off the mounted tree's tags, exactly as a dev
    // server reads it out of a patch it was handed.
    let site = session.overlay_site().expect("a tagged node in the mounted scene");
    let outcome = session.apply_overlay_patch(
        site,
        &[Edit::SetProp {
            node: 1,
            name: Cow::Borrowed("content"),
            value: LiteralValue::Str(Cow::Borrowed("after")),
        }],
    );
    assert_eq!(outcome.refused, 0);
    assert_eq!(outcome.applied, 1);

    assert_eq!(
        snapshot_texts(&recorder),
        vec!["after".to_string()],
        "the scene mirror must carry the patch, or a client connecting now gets the old text"
    );
    runtime_vocabulary::overlay::reset();
}
