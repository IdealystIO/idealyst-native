//! `app.net()`: hold, delay or fail the app's HTTP requests by URL — the
//! robot's `net.*` functions, typed. Every request the app sends through
//! the `net` SDK is matched, server functions included (`/_srv/<name>`).
//!
//! ```ignore
//! let search = app.net().hold("/_srv/*search");
//! app.test_id("search").type_text("copper");
//! app.net().wait_held(search);                 // the search is in flight
//! app.test_id("filter-status").click();        // the filter bar rebuilds
//! app.net().release(search);
//! app.test_id("search").assert_value("copper");
//! app.net().clear();
//! ```

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::{json, Value};

use crate::App;

/// A rule's id, from [`Net::hold`] and friends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetRule(pub u64);

/// A request a rule is holding.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct HeldRequest {
    pub request: u64,
    pub rule: u64,
    pub method: String,
    pub url: String,
}

/// The app's HTTP, through the robot. From [`App::net`].
pub struct Net<'a> {
    pub(crate) app: &'a mut App,
}

impl Net<'_> {
    fn intercept(&mut self, args: Value) -> NetRule {
        let v = self.app.call_function("net.intercept", args);
        NetRule(v["rule"].as_u64().unwrap_or_else(|| panic!("net.intercept: unexpected shape {v}")))
    }

    /// Hold requests whose URL contains `url` (`*` matches any run) until
    /// [`release`](Self::release).
    pub fn hold(&mut self, url: &str) -> NetRule {
        self.intercept(json!({ "url": url, "action": "hold" }))
    }

    /// Hold requests whose URL contains `url` for `by`, then send them.
    pub fn delay(&mut self, url: &str, by: Duration) -> NetRule {
        self.intercept(json!({ "url": url, "action": "delay", "ms": by.as_millis() as u64 }))
    }

    /// Answer requests whose URL contains `url` with HTTP `status` (and
    /// `body`), without sending them.
    pub fn fail_status(&mut self, url: &str, status: u16, body: &str) -> NetRule {
        self.intercept(json!({ "url": url, "action": "fail", "status": status, "body": body }))
    }

    /// Fail requests whose URL contains `url` with a transport error:
    /// `"network"`, `"timeout"` or `"offline"`.
    pub fn fail_error(&mut self, url: &str, error: &str) -> NetRule {
        self.intercept(json!({ "url": url, "action": "fail", "error": error }))
    }

    /// Add a rule with the full `net.intercept` arguments (`method`,
    /// `times`, …).
    pub fn intercept_with(&mut self, args: Value) -> NetRule {
        self.intercept(args)
    }

    /// The requests held now, oldest first.
    pub fn held(&mut self) -> Vec<HeldRequest> {
        let v = self.app.call_function("net.held", json!({}));
        serde_json::from_value(v.clone()).unwrap_or_else(|e| panic!("net.held: unexpected shape {v}: {e}"))
    }

    /// Wait (up to the app's timeout) until `rule` holds a request, and
    /// return the first. The app sends asynchronously after a click, so a
    /// test waits for the request to arrive before acting on it in flight.
    pub fn wait_held(&mut self, rule: NetRule) -> HeldRequest {
        let deadline = Instant::now() + self.app.timeout;
        loop {
            if let Some(h) = self.held().into_iter().find(|h| h.rule == rule.0) {
                return h;
            }
            if Instant::now() >= deadline {
                panic!("net: rule {} held no request within {:?}", rule.0, self.app.timeout);
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// Send on every request `rule` holds. Returns how many.
    pub fn release(&mut self, rule: NetRule) -> u64 {
        let v = self.app.call_function("net.release", json!({ "rule": rule.0 }));
        v["released"].as_u64().unwrap_or(0)
    }

    /// Answer every request `rule` holds with HTTP `status` instead.
    pub fn release_with_status(&mut self, rule: NetRule, status: u16, body: &str) -> u64 {
        let v = self.app.call_function("net.release", json!({ "rule": rule.0, "status": status, "body": body }));
        v["released"].as_u64().unwrap_or(0)
    }

    /// Remove a rule; what it holds stays held.
    pub fn remove(&mut self, rule: NetRule) {
        self.app.call_function("net.remove", json!({ "rule": rule.0 }));
    }

    /// Remove every rule and send every held request on.
    pub fn clear(&mut self) {
        self.app.call_function("net.clear", json!({}));
    }
}
