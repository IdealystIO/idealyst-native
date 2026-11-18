//! The per-process connection the `#[idea_test]` macro drives.
//!
//! All tests in a binary share **one** [`App`] connection, serialized behind a
//! mutex so the default parallel libtest harness can't interleave two tests
//! against the same live app. The connection is established lazily on the first
//! test and cached as a single ready-or-skip decision:
//!
//! - `IDEALYST_ROBOT_BRIDGE=host:port` set (by `idealyst test`) → connect there.
//! - else discover a running app under `~/.idealyst/apps`, preferring the one
//!   whose `project_root` matches this crate's `CARGO_MANIFEST_DIR` (the case
//!   where you ran `idealyst dev` in another terminal) — or, for a suite in
//!   a package of its own, the app its manifest names ([`suite_app_dir`]). An app an
//!   `idealyst test` run launched is never picked: that run drives it alone
//!   ([`discover`]).
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
/// After a test that passed, the screen it resized or turned is put back
/// here, and a failure to is that test's failure. After one that panicked
/// it is left for the next [`__acquire`].
impl Drop for AppGuard {
    fn drop(&mut self) {
        if !std::thread::panicking() {
            self.0.restore_screen();
        }
    }
}

impl std::ops::DerefMut for AppGuard {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.0
    }
}

/// Acquire the shared app for one test. Called by the `#[idea_test]` expansion;
/// not part of the public authoring surface.
#[doc(hidden)]
pub fn __acquire(_test: &str) -> Acquire {
    match CONN.get_or_init(establish) {
        Conn::Ready(m) => {
            // A panicking test poisons the mutex; recover the inner app so the
            // next test still runs rather than cascading false failures.
            let mut app = m.lock().unwrap_or_else(|p| p.into_inner());
            // A test that failed with the screen resized or turned left it
            // that way: its guard can't restore while unwinding (a second
            // panic there aborts the run). Put it back before this test.
            if app.screen_start.is_some() {
                let restored = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| app.restore_screen()));
                if restored.is_err() {
                    eprintln!("[idea_test] {_test}: the previous test's screen could not be put back (see above)");
                }
            }
            Acquire::Ready(AppGuard(app))
        }
        Conn::Skip(why) => Acquire::Skip(why.clone()),
    }
}

/// The platform names `#[idea_test(platforms = …)]` takes — the
/// spellings of `idealyst test --<platform>` and of the app's
/// `get_platform`.
pub const PLATFORMS: [&str; 6] = ["web", "ios", "android", "macos", "linux", "windows"];

/// The last path segment `#[idea_test(platforms = …)]` gives a test:
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

/// The prefix of the path segment `#[idea_test(suites = …)]` gives a test.
const SUITES_PREFIX: &str = "idea_suites_";

/// The path segment `#[idea_test(suites = …)]` gives a test:
/// `idea_suites_` and the suite names, sorted and deduplicated, joined by
/// `__` — `idea_suites_checkout__smoke`. It is the test's last segment
/// (`<fn>::idea_suites_smoke`), or the one before its platform marker
/// (`<fn>::idea_suites_smoke::robot_only_ios`).
pub fn suite_marker(suites: &[&str]) -> String {
    let mut sorted = suites.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    format!("{SUITES_PREFIX}{}", sorted.join("__"))
}

/// The suites a test belongs to, from its libtest name; empty for a test
/// in none. Reads the last segment, or the one before a `robot_only_…`
/// last segment (an author-ignored platform test's included).
pub fn suites_of_test(name: &str) -> Vec<&str> {
    let mut segments = name.rsplit("::");
    let Some(last) = segments.next() else { return Vec::new() };
    let segment = if last.starts_with("robot_only_") { segments.next() } else { Some(last) };
    segment
        .and_then(|s| s.strip_prefix(SUITES_PREFIX))
        .map(|rest| rest.split("__").filter(|s| !s.is_empty()).collect())
        .unwrap_or_default()
}

/// The deprecated `#[robot_test]` expands to a use of this, which is what
/// puts the deprecation warning on the caller's attribute.
#[doc(hidden)]
#[deprecated(note = "`#[robot_test]` is renamed `#[idea_test]` (same arguments): import `robot_test::idea_test`")]
pub const fn __robot_test_is_now_idea_test() {}

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
    // app registered for this very project, or the app a suite package names.
    let project = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(std::path::PathBuf::from)
        .map(|dir| suite_app_dir(&dir, test_target_name().as_deref()));
    discover_in(project.as_deref(), &apps_dirs)
        .ok_or_else(|| "no running app found (start `idealyst dev` or use `idealyst test`)".into())
}

/// The app a bare `cargo test` of the suite at `manifest_dir` drives: the
/// suite package's own directory, unless its manifest names an app in
/// `[package.metadata.idealyst.test] app` — one path for every target
/// (`app = "../app-main"`), or a table by test target when one package
/// holds the suites of several apps (`app = { main = "../app-main",
/// checkin = "../app-checkin" }`). Paths are relative to `manifest_dir`.
/// Without it a suite in its own package would match no app's
/// `project_root` and drive whichever app was running.
fn suite_app_dir(manifest_dir: &std::path::Path, target: Option<&str>) -> std::path::PathBuf {
    std::fs::read_to_string(manifest_dir.join("Cargo.toml"))
        .ok()
        .and_then(|manifest| named_app(&manifest, target))
        .map(|rel| manifest_dir.join(rel))
        .unwrap_or_else(|| manifest_dir.to_path_buf())
}

/// `[package.metadata.idealyst.test] app` from a manifest, for `target`.
fn named_app(manifest: &str, target: Option<&str>) -> Option<String> {
    let doc: toml::Table = manifest.parse().ok()?;
    let app = doc.get("package")?.get("metadata")?.get("idealyst")?.get("test")?.get("app")?;
    match app {
        toml::Value::String(path) => Some(path.clone()),
        toml::Value::Table(by_target) => by_target.get(target?)?.as_str().map(str::to_string),
        _ => None,
    }
}

/// This test binary's target name: cargo names it `<target>-<hash>`.
fn test_target_name() -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    target_of_exe(exe.file_stem()?.to_str()?)
}

fn target_of_exe(stem: &str) -> Option<String> {
    let (name, hash) = stem.rsplit_once('-')?;
    (hash.len() == 16 && hash.chars().all(|c| c.is_ascii_hexdigit())).then(|| name.to_string())
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
mod suite_tests {
    use super::*;

    /// The macro names a suite member `<fn>::idea_suites_<a>__<b>` (or puts
    /// that segment before its platform marker); the runner reads the
    /// suites back from the name.
    #[test]
    fn a_test_name_round_trips_its_suites() {
        assert_eq!(suite_marker(&["smoke", "checkout", "smoke"]), "idea_suites_checkout__smoke");
        assert_eq!(suites_of_test("pay::idea_suites_checkout__smoke"), ["checkout", "smoke"]);
        assert_eq!(suites_of_test("kb::idea_suites_smoke::robot_only_ios"), ["smoke"]);
        assert_eq!(suites_of_test("kb::idea_suites_smoke::robot_only_ios_ignored"), ["smoke"]);
        assert_eq!(suites_of_test("kb::idea_suites_check_out"), ["check_out"]);
        assert!(suites_of_test("kb::robot_only_ios").is_empty());
        assert!(suites_of_test("plain").is_empty());
        // Only the test's own segment counts, not an author's module.
        assert!(suites_of_test("idea_suites_smoke::helper").is_empty());
    }
}

#[cfg(test)]
mod locate_tests {
    use super::*;

    /// A suite in a package of its own names the app it drives; without
    /// that, its `CARGO_MANIFEST_DIR` matched no app and a bare `cargo test`
    /// drove whichever app was running.
    #[test]
    fn a_suite_package_finds_the_app_its_manifest_names() {
        let one = "[package]\nname = \"e2e\"\n[package.metadata.idealyst.test]\napp = \"../app-main\"\n";
        assert_eq!(named_app(one, Some("robot")).as_deref(), Some("../app-main"));
        assert_eq!(named_app(one, None).as_deref(), Some("../app-main"), "one app for every target");
        let per_target =
            "[package.metadata.idealyst.test.app]\nmain = \"../app-main\"\ncheckin = \"../app-checkin\"\n";
        assert_eq!(named_app(per_target, Some("checkin")).as_deref(), Some("../app-checkin"));
        assert_eq!(named_app(per_target, Some("other")), None);
        assert_eq!(named_app("[package]\nname = \"app\"\n", Some("robot")), None, "an app's own suite");

        let root = std::env::temp_dir().join(format!("robot-test-suite-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let e2e = root.join("robot-e2e");
        std::fs::create_dir_all(&e2e).unwrap();
        std::fs::write(e2e.join("Cargo.toml"), per_target).unwrap();
        assert_eq!(suite_app_dir(&e2e, Some("main")), e2e.join("../app-main"));
        assert_eq!(suite_app_dir(&e2e, Some("other")), e2e, "unnamed: the package itself, as before");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_test_binary_names_its_target() {
        assert_eq!(target_of_exe("robot-0123456789abcdef").as_deref(), Some("robot"));
        assert_eq!(target_of_exe("app-checkin-0123456789abcdef").as_deref(), Some("app-checkin"));
        assert_eq!(target_of_exe("robot"), None);
    }

    /// Regression: `#[idea_test]` searched only `~/.idealyst/apps`, so a
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
