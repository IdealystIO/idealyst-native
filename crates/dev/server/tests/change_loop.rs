//! The runtime-server host's change loop takes rebuild requests, and
//! under `idealyst dev --no-watch` holds saves until one arrives.
//!
//! Drives `spawn_change_loop_with` over a real temporary source tree: the
//! host feeds its `triggers` from the `rebuild` lines `idealyst dev`
//! writes to its stdin, and calls a rebuild for an empty path list.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use dev_server::spawn_change_loop_with;

const QUIET: Duration = Duration::from_millis(1500);
const PATIENT: Duration = Duration::from_secs(20);

fn tree() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().canonicalize().unwrap().join("src");
    std::fs::create_dir_all(&src).unwrap();
    (dir, src)
}

fn names(paths: &[PathBuf]) -> Vec<String> {
    let mut v: Vec<String> =
        paths.iter().filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn under_no_watch_saves_wait_for_a_rebuild_request() {
    let (_dir, src) = tree();
    let (trigger, triggers) = mpsc::channel();
    let (calls_tx, calls) = mpsc::channel::<Vec<PathBuf>>();
    let _loop = spawn_change_loop_with(
        vec![src.clone()],
        Duration::from_millis(100),
        true,
        triggers,
        Box::new(move |paths| {
            let _ = calls_tx.send(paths.to_vec());
        }),
    );
    std::thread::sleep(Duration::from_millis(500));

    std::fs::write(src.join("app.rs"), "fn a() {}\n").unwrap();
    std::fs::write(src.join("lib.rs"), "fn b() {}\n").unwrap();
    assert!(calls.recv_timeout(QUIET).is_err(), "a save reached the host under --no-watch");

    trigger.send(()).unwrap();
    let applied = calls.recv_timeout(PATIENT).expect("the request applied the queue");
    assert_eq!(names(&applied), vec!["app.rs", "lib.rs"]);

    // Nothing queued: called with no paths, which the host rebuilds.
    trigger.send(()).unwrap();
    let forced = calls.recv_timeout(PATIENT).expect("a request with nothing queued still calls");
    assert!(forced.is_empty(), "{forced:?}");
}

#[test]
fn without_no_watch_saves_apply_and_a_request_rebuilds() {
    let (_dir, src) = tree();
    let (trigger, triggers) = mpsc::channel();
    let (calls_tx, calls) = mpsc::channel::<Vec<PathBuf>>();
    let _loop = spawn_change_loop_with(
        vec![src.clone()],
        Duration::from_millis(100),
        false,
        triggers,
        Box::new(move |paths| {
            let _ = calls_tx.send(paths.to_vec());
        }),
    );
    std::thread::sleep(Duration::from_millis(500));

    std::fs::write(src.join("app.rs"), "fn a() {}\n").unwrap();
    let saved = calls.recv_timeout(PATIENT).expect("a save reached the host");
    assert!(names(&saved).contains(&"app.rs".to_string()), "{saved:?}");
    while calls.recv_timeout(QUIET).is_ok() {}

    trigger.send(()).unwrap();
    let forced = calls.recv_timeout(PATIENT).expect("the request reached the host");
    assert!(forced.is_empty(), "{forced:?}");
}
