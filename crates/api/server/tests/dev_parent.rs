//! A server built on `server::router()` exits when the `idealyst dev`
//! process that started it is gone (`server::dev_parent`).
//!
//! Each test re-runs this test binary as the "server": the child branch
//! of [`child_server`] builds the router, as a real server does at
//! startup, and then sits there. The test plays the dev process, holding
//! the write end of the child's stdin; dropping it is what the OS does
//! when the dev process is SIGKILLed.
#![cfg(feature = "server")]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CHILD_ENV: &str = "SERVER_DEV_PARENT_TEST_CHILD";

/// The child's half: build the router, then wait far longer than any
/// test does. Exits on its own only through the watch.
#[test]
fn child_server() {
    if std::env::var_os(CHILD_ENV).is_none() {
        return;
    }
    let _router = server::router();
    std::thread::sleep(Duration::from_secs(60));
}

fn spawn_child(watch: bool) -> Child {
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "child_server", "--nocapture", "--test-threads=1"])
        .env(CHILD_ENV, "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if watch {
        cmd.env(server::dev_parent::ENV, "stdin");
    } else {
        cmd.env_remove(server::dev_parent::ENV);
    }
    cmd.spawn().unwrap()
}

fn exited_within(child: &mut Child, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if child.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

/// Regression: an `idealyst dev` killed by the OOM killer left its
/// server re-parented to init, still holding the app's port, so the next
/// session could not bind it and the browser talked to a stale server.
#[test]
fn regression_server_outlived_a_killed_dev_session() {
    let mut child = spawn_child(true);
    // Up and watching, and not exiting while the "dev process" lives.
    assert!(!exited_within(&mut child, Duration::from_millis(1500)), "exited with the session still alive");
    drop(child.stdin.take());
    let gone = exited_within(&mut child, Duration::from_secs(10));
    if !gone {
        let _ = child.kill();
    }
    assert!(gone, "the server kept running after its dev session was gone");
    assert!(child.wait().unwrap().success());
}

/// Without the variable a closed stdin means nothing: production servers
/// commonly run with `/dev/null` or a closed stdin.
#[test]
fn a_server_not_started_by_dev_ignores_its_stdin() {
    let mut child = spawn_child(false);
    drop(child.stdin.take());
    let exited = exited_within(&mut child, Duration::from_millis(1500));
    let _ = child.kill();
    let _ = child.wait();
    assert!(!exited, "a closed stdin ended a server that dev did not start");
}
