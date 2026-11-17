//! Driving more than one app from one test: a second tab of the same web
//! app ([`App::open_page`]), any app on the same relay ([`App::apps`] /
//! [`App::select_app`]), or another project's running app
//! ([`App::connect_project`]). Each is its own [`App`], with its own
//! connection, so two can be held and driven in turn:
//!
//! ```ignore
//! let mut bob = app.open_page("/forms/claim");   // a second, signed-out tab
//! bob.test_id("email").type_text("bob@site");
//! app.test_id("claim").click();                   // alice claims
//! bob.test_id("claim").click();                   // bob is told it's taken
//!
//! let mut kiosk = App::connect_project("../app-checkin"); // another project's app
//! kiosk.test_id("pin").type_text("1234");
//! ```
//!
//! The project's running app is the suite's to start (`idealyst dev` in
//! another terminal, or a script beside `idealyst test`); `connect_project`
//! finds it the way the harness finds the main one.

use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use crate::client::{default_apps_dirs, discover_all, RobotClient};
use crate::App;

/// How long `open_page` waits for the new tab's app to boot (the relay's
/// own `load_url` wait).
const OPEN_PAGE_WAIT: Duration = Duration::from_secs(60);
/// How long `connect_project` waits for the project's app to register and
/// answer: a cold `idealyst dev` build.
const PROJECT_WAIT: Duration = Duration::from_secs(240);

/// An app connected to the same relay, from [`App::apps`].
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct RelayApp {
    pub id: u64,
    pub platform: Option<String>,
    pub label: Option<String>,
    /// The relay's default: the newest app not opened by `open_page`.
    pub active: bool,
    /// The app this connection drives.
    pub selected: bool,
}

impl App {
    /// Open another tab of this web app at `url` (a path resolves against
    /// this tab's address), in its own browser context: its own cookies and
    /// storage, so a second user signs in beside the first. Returns an
    /// [`App`] driving the new tab; this one keeps driving its own. Needs a
    /// browser the dev session launched (`idealyst test --web`).
    pub fn open_page(&mut self, url: &str) -> App {
        self.open_page_with(url, true)
    }

    /// [`open_page`](Self::open_page) in this tab's browser context: the
    /// new tab shares its cookies (the same signed-in user).
    pub fn open_page_shared(&mut self, url: &str) -> App {
        self.open_page_with(url, false)
    }

    fn open_page_with(&mut self, url: &str, isolated: bool) -> App {
        // Firefox / WebKit: the runner opens the tab; the relay is told to
        // take the next page as a secondary app, as its own `open_page` does.
        if let Some(control) = crate::browser_control::runner_answers_browser_verbs() {
            let before = self.newest_app_id();
            self.call("expect_secondary", json!({}));
            if let Err(e) = crate::browser_control::request(control, "open_page", json!({ "url": url, "isolated": isolated })) {
                self.call("expect_secondary", json!({ "cancel": true }));
                panic!("open_page({url:?}): {e}");
            }
            let id = self
                .wait_for_new_app(before, OPEN_PAGE_WAIT, true)
                .unwrap_or_else(|e| panic!("open_page({url:?}): {e}"));
            let mut other = App::connect(self.bridge_addr())
                .unwrap_or_else(|e| panic!("open_page: a second connection to the relay: {e}"));
            other.timeout = self.timeout;
            other.select_app(id);
            return other;
        }
        let io = self.client_io_timeout();
        self.set_io_timeout(OPEN_PAGE_WAIT + Duration::from_secs(5));
        // The relay waits as long as this connection does (`timeout_ms`).
        let opened = self.try_call("open_page", json!({ "url": url, "isolated": isolated }));
        self.set_io_timeout(io);
        let opened = opened.unwrap_or_else(|e| panic!("open_page({url:?}): {e}"));
        let id = opened["app"].as_u64().unwrap_or_else(|| panic!("open_page: unexpected shape {opened}"));
        let mut other = App::connect(self.bridge_addr())
            .unwrap_or_else(|e| panic!("open_page: a second connection to the relay: {e}"));
        other.timeout = self.timeout;
        other.select_app(id);
        other
    }

    /// The apps connected to this app's relay, oldest first.
    pub fn apps(&mut self) -> Vec<RelayApp> {
        let v = self.call("apps", json!({}));
        serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("apps: unexpected shape {v}: {e}"))
    }

    /// Drive app `id` (from [`apps`](Self::apps)) from this connection;
    /// `None` goes back to the relay's default.
    pub fn select_app(&mut self, id: impl Into<Option<u64>>) {
        let id = id.into();
        self.call("select_app", json!({ "app": id }));
    }

    /// Connect to the running app of the project at `dir` (another
    /// project's `idealyst dev` session: a kiosk beside the main app),
    /// waiting for it to register and answer.
    pub fn connect_project(dir: impl AsRef<Path>) -> App {
        let dir = dir.as_ref();
        let deadline = Instant::now() + PROJECT_WAIT;
        let addr: SocketAddr = loop {
            if let Some(addr) =
                default_apps_dirs().iter().find_map(|apps| discover_all(Some(dir), apps).into_iter().next())
            {
                break addr;
            }
            assert!(
                Instant::now() < deadline,
                "connect_project({}): no running app registered for it (start `idealyst dev` there)",
                dir.display()
            );
            std::thread::sleep(Duration::from_millis(250));
        };
        let mut client = RobotClient::connect(addr)
            .unwrap_or_else(|e| panic!("connect_project({}): {addr}: {e}", dir.display()));
        let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_secs(5));
        client
            .wait_ready(left)
            .unwrap_or_else(|e| panic!("connect_project({}): {e}", dir.display()));
        App::from_client(client)
    }
}
