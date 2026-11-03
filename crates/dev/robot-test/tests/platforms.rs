//! `#[robot_test(platforms = …)]` end to end: real platform-bound tests,
//! the harness connecting to a mock bridge that reports its platform, and
//! the test binary's own `--list` — what `idealyst test` selects from.
//!
//! The bound tests are `#[ignore]`d by the macro itself and called from
//! one driver test: the harness decides connect-or-skip once per binary,
//! so the bridge address must be in the environment before they run.

use robot_test::{json, robot_test, App, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

#[robot_test(platforms = [ios, android])]
fn mobile_only(app: &mut App) {
    app.call_function("mobile body", json!({}));
}

#[robot_test(platforms = web)]
fn web_only(app: &mut App) {
    app.call_function("web body", json!({}));
}

/// Against a web app: the web test runs, the mobile one skips without
/// touching the app — and a body bound to a platform still reaches the
/// file's items (`json!`, `App`) through the module's `use super::*`.
#[test]
fn a_platform_bound_test_runs_only_on_its_platform() {
    let (addr, calls) = spawn_bridge("web");
    std::env::set_var("IDEALYST_ROBOT_BRIDGE", addr.to_string());

    web_only::robot_only_web();
    mobile_only::robot_only_ios_android();
    assert_eq!(*calls.lock().unwrap(), ["web body"]);
}

/// Bare `cargo test` reports them ignored, under names that say where
/// they run — never as passed.
#[test]
fn the_binary_lists_them_ignored_under_their_platform_names() {
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--list", "--ignored", "--format", "terse"])
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&out.stdout);
    let mut names: Vec<&str> = listed.lines().filter_map(|l| l.strip_suffix(": test")).collect();
    names.sort();
    assert_eq!(names, ["mobile_only::robot_only_ios_android", "web_only::robot_only_web"]);
    for name in names {
        assert!(robot_test::platforms_of_test(name).is_some(), "{name}");
    }
}

/// A bridge answering `ping`, `get_platform` with `platform`, and
/// recording each `call_function` by name.
fn spawn_bridge(platform: &'static str) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let log = log.clone();
            std::thread::spawn(move || serve(conn, platform, log));
        }
    });
    (addr, calls)
}

fn serve(stream: TcpStream, platform: &str, log: Arc<Mutex<Vec<String>>>) {
    let mut writer = stream.try_clone().unwrap();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let req: Value = serde_json::from_str(&line).unwrap();
        let ok = match req["cmd"].as_str() {
            Some("ping") => json!("pong"),
            Some("get_platform") => json!({ "platform": platform }),
            Some("call_function") => {
                log.lock().unwrap().push(req["args"]["name"].as_str().unwrap_or("").to_string());
                json!({ "done": true, "value": null })
            }
            _ => Value::Null,
        };
        let reply = json!({ "id": req["id"], "ok": ok }).to_string() + "\n";
        if writer.write_all(reply.as_bytes()).is_err() {
            break;
        }
    }
}
