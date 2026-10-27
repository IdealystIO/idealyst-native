//! The runtime-server host's change loop reports a numbered rebuild
//! request taken before it applies it. Its own test binary: it installs
//! the process-wide event reporter, which only the first caller can.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

const PATIENT: Duration = Duration::from_secs(20);

fn tree() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().canonicalize().unwrap().join("src");
    std::fs::create_dir_all(&src).unwrap();
    (dir, src)
}

/// A numbered request is reported taken before the host applies it, so a
/// wait on that request knows which of the host's outcomes answers it.
#[test]
fn a_numbered_request_is_reported_taken_before_it_is_applied() {
    let events = dev_events::Queue::new();
    let reporter = dev_events::Reporter::new();
    reporter.add_sink(std::sync::Arc::new(events.clone()));
    assert!(dev_events::install_global(reporter), "this test owns the process's reporter");
    let (_dir, src) = tree();
    let (trigger, triggers) = mpsc::channel();
    let (calls_tx, calls) = mpsc::channel::<Vec<dev_events::DevEvent>>();
    let seen = events.clone();
    let _loop = dev_server::spawn_change_loop_numbered(
        vec![src.clone()],
        Duration::from_millis(100),
        false,
        triggers,
        Box::new(move |_| {
            let _ = calls_tx.send(seen.drain().into_iter().map(|e| e.event).collect());
        }),
    );
    trigger.send(Some(4)).unwrap();
    let before = calls.recv_timeout(PATIENT).expect("the request reached the host");
    assert!(
        before.iter().any(|e| matches!(e, dev_events::DevEvent::RebuildTaken { request: 4, .. })),
        "{before:?}"
    );
}
