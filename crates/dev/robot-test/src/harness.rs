//! The per-process connection the `#[robot_test]` macro drives.
//!
//! All tests in a binary share **one** [`App`] connection, serialized behind a
//! mutex so the default parallel libtest harness can't interleave two tests
//! against the same live app. The connection is established lazily on the first
//! test and cached as a single ready-or-skip decision:
//!
//! - `IDEALYST_ROBOT_BRIDGE=host:port` set (by `idealyst test`) → connect there.
//! - else discover a running app under `~/.idealyst/apps`, preferring the one
//!   whose `project_root` matches this crate's `CARGO_MANIFEST_DIR` (the case
//!   where you ran `idealyst dev` in another terminal).
//! - else **skip** — no app is reachable, so every test prints a skip note and
//!   returns. This is what keeps a bare `cargo test` (with no app up) green
//!   instead of red.

use crate::app::App;
use crate::client::{default_apps_dir, discover, RobotClient};
use std::net::SocketAddr;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

/// How long to wait for the app to answer a ping once we've found a bridge.
/// `idealyst test` already waited for readiness before launching the tests, so
/// this is mostly a safety margin for the bare-`cargo test` path.
const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// The cached connection decision, made once per test binary.
enum Conn {
    Ready(Mutex<App>),
    Skip(String),
}

static CONN: OnceLock<Conn> = OnceLock::new();

/// What the macro-generated `#[test]` gets back from [`__acquire`].
pub enum Acquire {
    /// The app is connected; the guard derefs to [`App`] and serializes access.
    Ready(AppGuard),
    /// No app reachable — the test should print a note and return.
    Skip(String),
}

/// A locked handle to the shared [`App`]. Held for the duration of one test.
pub struct AppGuard(MutexGuard<'static, App>);

impl std::ops::Deref for AppGuard {
    type Target = App;
    fn deref(&self) -> &App {
        &self.0
    }
}
impl std::ops::DerefMut for AppGuard {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.0
    }
}

/// Acquire the shared app for one test. Called by the `#[robot_test]` expansion;
/// not part of the public authoring surface.
#[doc(hidden)]
pub fn __acquire(_test: &str) -> Acquire {
    match CONN.get_or_init(establish) {
        Conn::Ready(m) => Acquire::Ready(AppGuard(
            // A panicking test poisons the mutex; recover the inner app so the
            // next test still runs rather than cascading false failures.
            m.lock().unwrap_or_else(|p| p.into_inner()),
        )),
        Conn::Skip(why) => Acquire::Skip(why.clone()),
    }
}

fn establish() -> Conn {
    let addr = match locate_bridge() {
        Ok(a) => a,
        Err(why) => return Conn::Skip(why),
    };
    match RobotClient::connect(addr) {
        Ok(mut client) => match client.wait_ready(READY_TIMEOUT) {
            Ok(()) => Conn::Ready(Mutex::new(App::from_client(client))),
            Err(e) => Conn::Skip(format!("app at {addr} never became ready: {e}")),
        },
        Err(e) => Conn::Skip(format!("could not connect to the bridge at {addr}: {e}")),
    }
}

/// Find the bridge address: explicit env var first, then discovery.
fn locate_bridge() -> Result<SocketAddr, String> {
    if let Some(raw) = std::env::var_os("IDEALYST_ROBOT_BRIDGE") {
        let raw = raw.to_string_lossy();
        return raw
            .parse::<SocketAddr>()
            .map_err(|e| format!("IDEALYST_ROBOT_BRIDGE={raw:?} is not a host:port: {e}"));
    }

    let apps_dir =
        default_apps_dir().ok_or_else(|| "no HOME, so no ~/.idealyst/apps to search".to_string())?;
    // Cargo sets CARGO_MANIFEST_DIR for the test binary's package; prefer the
    // app registered for this very project.
    let project = std::env::var_os("CARGO_MANIFEST_DIR").map(std::path::PathBuf::from);
    discover(project.as_deref(), &apps_dir)
        .ok_or_else(|| "no running app found (start `idealyst dev` or use `idealyst test`)".into())
}
