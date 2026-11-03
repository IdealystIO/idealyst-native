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
use crate::client::{default_apps_dirs, discover, RobotClient};
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

/// The platform names `#[robot_test(platforms = …)]` takes — the
/// spellings of `idealyst test --<platform>` and of the app's
/// `get_platform`.
pub const PLATFORMS: [&str; 6] = ["web", "ios", "android", "macos", "linux", "windows"];

/// The last path segment `#[robot_test(platforms = …)]` gives a test:
/// `robot_only_` and the platforms, in [`PLATFORMS`] order, joined by
/// `_` — `robot_only_ios_android`. The test's libtest name is
/// `<fn>::robot_only_ios_android`, which is how a runner that only has
/// the test binary's `--list` learns where it runs.
pub fn platform_marker(platforms: &[&str]) -> String {
    let mut ordered: Vec<&str> = PLATFORMS.iter().copied().filter(|p| platforms.contains(p)).collect();
    ordered.dedup();
    format!("robot_only_{}", ordered.join("_"))
}

/// The platforms a test declared, from its libtest name; `None` for a
/// test that runs everywhere. Strict: only a last segment made exactly
/// of `robot_only_` and known platform names counts.
pub fn platforms_of_test(name: &str) -> Option<Vec<&'static str>> {
    let leaf = name.rsplit("::").next()?;
    let rest = leaf.strip_prefix("robot_only_")?;
    let mut out = Vec::new();
    for part in rest.split('_') {
        out.push(*PLATFORMS.iter().find(|p| **p == part)?);
    }
    (!out.is_empty()).then_some(out)
}

/// [`__acquire`] for a test that declared its platforms: on any other
/// platform it is skipped. `idealyst test` never runs it there (it is
/// `#[ignore]`d and only included where it belongs); this check covers
/// `cargo test -- --include-ignored` against whatever app is up.
#[doc(hidden)]
pub fn __acquire_on(test: &str, platforms: &[&str]) -> Acquire {
    match __acquire(test) {
        Acquire::Ready(mut app) => match app.try_platform() {
            Ok(Some(p)) if !platforms.contains(&p.as_str()) => {
                Acquire::Skip(format!("it runs on {} only, and the app is {p}", platforms.join(", ")))
            }
            Ok(_) => Acquire::Ready(app),
            // Up but not answering: a failure, never a quiet pass.
            Err(e) => panic!("{test}: the app did not say which platform it is: {e}"),
        },
        skip => skip,
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

    let apps_dirs = default_apps_dirs();
    if apps_dirs.is_empty() {
        return Err("no HOME, so no ~/.idealyst/apps to search".to_string());
    }
    // Cargo sets CARGO_MANIFEST_DIR for the test binary's package; prefer the
    // app registered for this very project.
    let project = std::env::var_os("CARGO_MANIFEST_DIR").map(std::path::PathBuf::from);
    discover_in(project.as_deref(), &apps_dirs)
        .ok_or_else(|| "no running app found (start `idealyst dev` or use `idealyst test`)".into())
}

/// The first registry in `apps_dirs` holding a live app (see [`discover`]).
/// Every registry, not only `~/.idealyst/apps`: a Windows app under Wine
/// registers in `apps-wine` beside it (`default_apps_dirs`).
fn discover_in(project: Option<&std::path::Path>, apps_dirs: &[std::path::PathBuf]) -> Option<SocketAddr> {
    apps_dirs.iter().find_map(|d| discover(project, d))
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    /// The macro names a platform-bound test `<fn>::robot_only_<platforms>`;
    /// the runner reads the platforms back from that name.
    #[test]
    fn a_test_name_round_trips_its_platforms() {
        assert_eq!(platform_marker(&["android", "ios"]), "robot_only_ios_android");
        assert_eq!(platforms_of_test("kb::robot_only_ios_android"), Some(vec!["ios", "android"]));
        assert_eq!(platforms_of_test("suite::kb::robot_only_web"), Some(vec!["web"]));
        assert_eq!(platforms_of_test("kb"), None);
        assert_eq!(platforms_of_test("robot_only_"), None);
        // An authored `#[ignore]` unmarks the name.
        assert_eq!(platforms_of_test("kb::robot_only_ios_ignored"), None);
        assert_eq!(platforms_of_test("robot_only_ios::helper"), None);
    }
}

#[cfg(test)]
mod locate_tests {
    use super::*;

    /// Regression: `#[robot_test]` searched only `~/.idealyst/apps`, so a
    /// Windows app under Wine (registered in `apps-wine`) was never found,
    /// although `idealyst test` searched both.
    #[test]
    fn regression_harness_finds_an_app_in_the_wine_registry() {
        let home = std::env::temp_dir().join(format!("robot-test-locate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let native = home.join("apps");
        let wine = home.join(crate::client::WINE_APPS_DIR_NAME);
        std::fs::create_dir_all(&native).unwrap();
        std::fs::create_dir_all(&wine).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::fs::write(
            wine.join("winapp-32.json"),
            format!(r#"{{"port":{port},"project_root":""}}"#),
        )
        .unwrap();
        let found = discover_in(None, &[native, wine]);
        assert_eq!(found.map(|a| a.port()), Some(port));
        let _ = std::fs::remove_dir_all(&home);
    }
}
