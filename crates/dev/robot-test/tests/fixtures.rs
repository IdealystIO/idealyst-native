//! `#[robot_test]` fixtures end to end: real `#[robot_test]` functions, the
//! harness connecting to a mock bridge through `IDEALYST_ROBOT_BRIDGE`, and
//! fixtures set up before the body and torn down after it — in reverse
//! order, and also when the body panics.
//!
//! The fixture tests are `#[ignore]`d and called from one driver test: the
//! harness decides connect-or-skip once per binary, so the bridge address
//! must be in the environment before the first of them runs.

use robot_test::{json, robot_test, App, Fixture, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

/// A clean server (the app's `test.reset`).
struct FreshServer;

impl Fixture for FreshServer {
    fn setup(app: &mut App) -> Self {
        app.call_function("test.reset", json!({}));
        FreshServer
    }
}

/// Signed in — on a fresh server — and signed out afterwards.
struct SignedIn {
    email: String,
}

impl Fixture for SignedIn {
    fn setup(app: &mut App) -> Self {
        FreshServer::setup(app);
        let email = "ada@example.com".to_string();
        app.call_function("test.sign_in", json!({ "email": email }));
        SignedIn { email }
    }

    fn teardown(&mut self, app: &mut App) {
        app.call_function("test.sign_out", json!({ "email": self.email }));
    }
}

/// Rows the body may add to, removed afterwards.
struct Seeded {
    rows: u32,
}

impl Fixture for Seeded {
    fn setup(app: &mut App) -> Self {
        app.call_function("test.seed", json!({ "rows": 2 }));
        Seeded { rows: 2 }
    }

    fn teardown(&mut self, app: &mut App) {
        app.call_function("test.unseed", json!({ "rows": self.rows }));
    }
}

#[robot_test]
#[ignore = "run by `fixtures_wrap_the_test_body`"]
fn uses_two_fixtures(app: &mut App, user: &SignedIn, rows: &mut Seeded) {
    assert_eq!(user.email, "ada@example.com");
    rows.rows += 1;
    app.call_function("body", json!({}));
}

#[robot_test]
#[ignore = "run by `fixtures_wrap_the_test_body`"]
fn fails_while_signed_in(app: &mut App, _: &SignedIn) {
    app.call_function("body", json!({}));
    panic!("the body failed");
}

#[test]
fn fixtures_wrap_the_test_body() {
    let (addr, calls) = spawn_bridge();
    std::env::set_var("IDEALYST_ROBOT_BRIDGE", addr.to_string());

    uses_two_fixtures();
    assert_eq!(
        take(&calls),
        [
            r#"test.reset {}"#,
            r#"test.sign_in {"email":"ada@example.com"}"#,
            r#"test.seed {"rows":2}"#,
            r#"body {}"#,
            // Reverse order; the body's change to its `&mut` fixture is seen.
            r#"test.unseed {"rows":3}"#,
            r#"test.sign_out {"email":"ada@example.com"}"#,
        ]
    );

    let failed = std::panic::catch_unwind(fails_while_signed_in);
    let message = failed.unwrap_err();
    assert_eq!(message.downcast_ref::<&str>(), Some(&"the body failed"), "the body's own panic surfaces");
    assert_eq!(
        take(&calls).last().map(String::as_str),
        Some(r#"test.sign_out {"email":"ada@example.com"}"#),
        "torn down after a failure too"
    );
}

fn take(calls: &Mutex<Vec<String>>) -> Vec<String> {
    std::mem::take(&mut *calls.lock().unwrap())
}

/// A bridge answering `ping` and recording each `call_function` as
/// `"name args"`.
fn spawn_bridge() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let log = log.clone();
            std::thread::spawn(move || serve(conn, log));
        }
    });
    (addr, calls)
}

fn serve(stream: TcpStream, log: Arc<Mutex<Vec<String>>>) {
    let mut writer = stream.try_clone().unwrap();
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { break };
        let req: Value = serde_json::from_str(&line).unwrap();
        let ok = match req["cmd"].as_str() {
            Some("ping") => json!("pong"),
            Some("call_function") => {
                let name = req["args"]["name"].as_str().unwrap_or("");
                log.lock().unwrap().push(format!("{name} {}", req["args"]["args"]));
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
