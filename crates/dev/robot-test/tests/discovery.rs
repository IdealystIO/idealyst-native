//! A launcher waiting for its own app must not settle for another project's.
//!
//! `idealyst test` waited for "a registration for this project" with
//! `discover`, which falls back to ANY live app when none matches. With
//! another `idealyst dev` session running (two idea-ui-docs sessions, in the
//! run that found this), it took that session's relay before its own app had
//! registered, pinged an app that would never answer, and failed after four
//! minutes with "the app did not become ready" — on every platform.

use robot_test::{discover, discover_project};
use std::net::TcpListener;

fn register(dir: &std::path::Path, name: &str, root: &std::path::Path) -> TcpListener {
    let live = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = live.local_addr().unwrap().port();
    let reg = serde_json::json!({ "name": name, "pid": 1, "port": port, "project_root": root });
    std::fs::write(dir.join(format!("{name}.json")), reg.to_string()).unwrap();
    live
}

#[test]
fn regression_discover_project_never_falls_back_to_another_app() {
    let apps = std::env::temp_dir().join(format!("robot-test-discovery-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&apps);
    std::fs::create_dir_all(&apps).unwrap();
    let mine = apps.join("mine");
    let other = apps.join("other");
    std::fs::create_dir_all(&mine).unwrap();
    std::fs::create_dir_all(&other).unwrap();

    let _other_live = register(&apps, "other", &other);
    assert!(discover(Some(&mine), &apps).is_some(), "`discover` falls back to the live other app");
    assert_eq!(discover_project(&mine, &apps), None, "but a launcher must keep waiting for its own");

    let live = register(&apps, "mine", &mine);
    let addr = discover_project(&mine, &apps).expect("its own app, once registered");
    assert_eq!(addr.port(), live.local_addr().unwrap().port());
    let _ = std::fs::remove_dir_all(&apps);
}
