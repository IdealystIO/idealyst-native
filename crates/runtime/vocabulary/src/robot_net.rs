//! The robot's `net.*` functions: hold, delay or fail the app's HTTP
//! requests by URL, so a test can put the app in the moment between a
//! request leaving and its answer (a search in flight while its filter
//! bar rebuilds), or make the server fail.
//!
//! They set `test-net` rules, which the `net` SDK consults for every
//! request it sends (server functions included) before the network. A
//! held request is listed by `net.held` and goes on (or fails) at
//! `net.release`. A delay is a hold this module releases on the
//! framework's timer once its time is up, which works on every backend
//! (`test-net` has no clock or threads, so it can run on the web).
//!
//! Registered by the robot itself, like `clock.*`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use runtime_shared::__serde_json::{json, Value};
use runtime_shared::scheduling::{after_ms, ScheduledTask};

use crate::robot_fn::{register, RobotArg, RobotFunction, RobotReply};

/// How often a delay's hold is checked, ms.
const DELAY_TICK_MS: i32 = 10;

runtime_tls::thread_local! {
    /// Delay rules: rule id → how long its requests are held, µs.
    static DELAYS: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
    /// When each delayed request was first seen held, µs.
    static SEEN: RefCell<HashMap<u64, u64>> = RefCell::new(HashMap::new());
    static TICKER: RefCell<Option<ScheduledTask>> = const { RefCell::new(None) };
    static IN_TICK: Cell<bool> = const { Cell::new(false) };
}

fn arg<T: serde::de::DeserializeOwned>(args: &Value, function: &str, name: &str) -> Result<T, String> {
    crate::robot_fn::__arg(args, function, name)
}

/// A failure from `{status, body?}` or `{error}`; `None` when neither.
fn failure(args: &Value, function: &str) -> Result<Option<test_net::Failure>, String> {
    let status: Option<u16> = arg(args, function, "status")?;
    let error: Option<String> = arg(args, function, "error")?;
    let body: Option<String> = arg(args, function, "body")?;
    Ok(match (status, error.as_deref()) {
        (Some(_), Some(_)) => return Err(format!("{function}: give `status` or `error`, not both")),
        (Some(code), None) => Some(test_net::Failure::Status { code, body: body.unwrap_or_default() }),
        (None, Some("network")) => Some(test_net::Failure::Network("faked by the robot".into())),
        (None, Some("timeout")) => Some(test_net::Failure::Timeout),
        (None, Some("offline")) => Some(test_net::Failure::Offline),
        (None, Some(other)) => return Err(format!("{function}: unknown error '{other}' (network|timeout|offline)")),
        (None, None) => None,
    })
}

fn held_json() -> Value {
    Value::Array(
        test_net::held()
            .into_iter()
            .map(|h| json!({ "request": h.id, "rule": h.rule, "method": h.method, "url": h.url }))
            .collect(),
    )
}

/// Release every delayed request whose time is up, as of `now` (µs).
/// Returns whether any delay rule is still in force.
fn release_due(now: u64) -> bool {
    let delays = DELAYS.with(|d| d.borrow().clone());
    let held = test_net::held();
    SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        seen.retain(|id, _| held.iter().any(|h| h.id == *id));
        for h in held.iter() {
            let Some(&wait) = delays.get(&h.rule) else { continue };
            let since = *seen.entry(h.id).or_insert(now);
            if now.saturating_sub(since) >= wait {
                test_net::release(test_net::Which::Request(h.id), None);
                seen.remove(&h.id);
            }
        }
    });
    !delays.is_empty()
}

/// Keep checking the delays while any are in force. Without a scheduler
/// (`after_ms` then runs at once) the re-entrant call stops instead of
/// recursing; tests drive [`release_due`] by hand.
fn tick() {
    if IN_TICK.with(|t| t.replace(true)) {
        return;
    }
    let again = release_due(runtime_shared::time::now_micros());
    let task = again.then(|| after_ms(DELAY_TICK_MS, tick));
    IN_TICK.with(|t| t.set(false));
    TICKER.with(|t| *t.borrow_mut() = task);
}

fn start_ticker() {
    if TICKER.with(|t| t.borrow().is_none()) {
        tick();
    }
}

fn function(
    name: &'static str,
    doc: &'static str,
    args: &'static [RobotArg],
    returns: &'static str,
    call: impl Fn(&Value) -> Result<Value, String> + 'static,
) -> RobotFunction {
    RobotFunction {
        name,
        doc,
        args,
        returns,
        is_async: false,
        call: Some(Rc::new(move |a: &Value| call(a).map(RobotReply::Ready))),
    }
}

pub(crate) fn register_all() {
    register(function(
        "net.intercept",
        "Add a rule for the app's HTTP requests whose URL contains `url` (`*` matches any run: \
         `/_srv/*search`), of `method` if given. `action`: `hold` (parked until `net.release`), \
         `delay` (held `ms`, then sent), or `fail` (answered at once with `status` [+ `body`], or \
         `error`: network|timeout|offline; never sent). `times`: apply to that many requests, then \
         retire. The newest matching rule wins. Returns {rule}.",
        &[
            RobotArg { name: "url", ty: "String" },
            RobotArg { name: "method", ty: "Option<String>" },
            RobotArg { name: "action", ty: "String" },
            RobotArg { name: "ms", ty: "Option<u64>" },
            RobotArg { name: "status", ty: "Option<u16>" },
            RobotArg { name: "body", ty: "Option<String>" },
            RobotArg { name: "error", ty: "Option<String>" },
            RobotArg { name: "times", ty: "Option<u32>" },
        ],
        "{rule}",
        |a| {
            let f = "net.intercept";
            let url: String = arg(a, f, "url")?;
            let method: Option<String> = arg(a, f, "method")?;
            let action: String = arg(a, f, "action")?;
            let times: Option<u32> = arg(a, f, "times")?;
            let (rule_action, delay) = match action.as_str() {
                "hold" => (test_net::Action::Hold, None),
                "delay" => {
                    let ms: u64 = arg::<Option<u64>>(a, f, "ms")?.ok_or("net.intercept: `delay` needs `ms`")?;
                    (test_net::Action::Hold, Some(ms))
                }
                "fail" => (
                    test_net::Action::Fail(failure(a, f)?.ok_or("net.intercept: `fail` needs `status` or `error`")?),
                    None,
                ),
                other => return Err(format!("net.intercept: unknown action '{other}' (hold|delay|fail)")),
            };
            let rule = test_net::add_rule(test_net::Rule { url, method, action: rule_action, times });
            if let Some(ms) = delay {
                DELAYS.with(|d| d.borrow_mut().insert(rule, ms.saturating_mul(1000)));
                start_ticker();
            }
            Ok(json!({ "rule": rule }))
        },
    ));
    register(function(
        "net.held",
        "The requests held now, oldest first: [{request, rule, method, url}].",
        &[],
        "[{request, rule, method, url}]",
        |_| Ok(held_json()),
    ));
    register(function(
        "net.release",
        "Let held requests go: the one `request`, every one held by `rule`, or all of them. They \
         are sent on, or answered with `status` [+ `body`] / `error` (network|timeout|offline) \
         if given. Returns {released}.",
        &[
            RobotArg { name: "request", ty: "Option<u64>" },
            RobotArg { name: "rule", ty: "Option<u64>" },
            RobotArg { name: "status", ty: "Option<u16>" },
            RobotArg { name: "body", ty: "Option<String>" },
            RobotArg { name: "error", ty: "Option<String>" },
        ],
        "{released}",
        |a| {
            let f = "net.release";
            let request: Option<u64> = arg(a, f, "request")?;
            let rule: Option<u64> = arg(a, f, "rule")?;
            let which = match (request, rule) {
                (Some(_), Some(_)) => return Err("net.release: give `request` or `rule`, not both".into()),
                (Some(id), None) => test_net::Which::Request(id),
                (None, Some(id)) => test_net::Which::Rule(id),
                (None, None) => test_net::Which::All,
            };
            Ok(json!({ "released": test_net::release(which, failure(a, f)?) }))
        },
    ));
    register(function(
        "net.remove",
        "Remove a rule; requests it already holds stay held until released.",
        &[RobotArg { name: "rule", ty: "u64" }],
        "{removed}",
        |a| {
            let rule: u64 = arg(a, "net.remove", "rule")?;
            DELAYS.with(|d| d.borrow_mut().remove(&rule));
            Ok(json!({ "removed": test_net::remove_rule(rule) }))
        },
    ));
    register(function(
        "net.clear",
        "Remove every rule and send every held request on.",
        &[],
        "()",
        |_| {
            DELAYS.with(|d| d.borrow_mut().clear());
            SEEN.with(|s| s.borrow_mut().clear());
            TICKER.with(|t| t.borrow_mut().take());
            test_net::clear();
            Ok(Value::Null)
        },
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::robot::bridge::invoke_command;
    use runtime_shared::__serde_json as serde_json;

    fn call(name: &str, args: Value) -> Result<Value, String> {
        let reply = invoke_command("call_function", &json!({ "name": name, "args": args }))?;
        let v: Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
        Ok(v["value"].clone())
    }

    fn poll(f: &mut test_net::HoldFuture) -> std::task::Poll<Option<test_net::Failure>> {
        std::pin::Pin::new(f).poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
    }
    use std::future::Future;

    /// want_998196cd (CrewForge): hold the search request in flight, see
    /// it listed, release it; delay one; fail one. The rules are the
    /// robot's own functions, nothing to register. (Their effect on real
    /// requests is `net`'s `test_net` test.)
    #[test]
    fn a_test_holds_delays_and_fails_requests_through_the_robot() {
        call("net.clear", json!({})).unwrap();
        let rule = call("net.intercept", json!({ "url": "/_srv/*search", "action": "hold" })).unwrap()["rule"]
            .as_u64()
            .unwrap();
        let test_net::Decision::Hold(mut h) = test_net::intercept("POST", "http://h/_srv/project_search") else {
            panic!("held")
        };
        let held = call("net.held", json!({})).unwrap();
        assert_eq!(held[0]["rule"], rule);
        assert_eq!(held[0]["method"], "POST");
        let out = call("net.release", json!({ "rule": rule, "status": 500 })).unwrap();
        assert_eq!(out["released"], 1);
        assert!(matches!(poll(&mut h), std::task::Poll::Ready(Some(test_net::Failure::Status { code: 500, .. }))));

        // A delay is a hold released once its time is up.
        call("net.clear", json!({})).unwrap();
        call("net.intercept", json!({ "url": "/slow", "action": "delay", "ms": 300 })).unwrap();
        let test_net::Decision::Hold(mut d) = test_net::intercept("GET", "http://h/slow") else { panic!("held") };
        assert!(release_due(1_000_000), "a delay rule is in force");
        assert!(poll(&mut d).is_pending(), "first seen now");
        release_due(1_000_000 + 299_000);
        assert!(poll(&mut d).is_pending(), "not yet");
        release_due(1_000_000 + 300_000);
        assert_eq!(poll(&mut d), std::task::Poll::Ready(None), "time is up: sent");

        call("net.intercept", json!({ "url": "/save", "action": "fail", "error": "offline" })).unwrap();
        assert!(matches!(test_net::intercept("POST", "http://h/save"), test_net::Decision::Fail(test_net::Failure::Offline)));

        for (args, needle) in [
            (json!({ "url": "/x", "action": "fly" }), "unknown action"),
            (json!({ "url": "/x", "action": "delay" }), "needs `ms`"),
            (json!({ "url": "/x", "action": "fail" }), "needs `status` or `error`"),
            (json!({ "url": "/x", "action": "fail", "status": 500, "error": "offline" }), "not both"),
        ] {
            let err = call("net.intercept", args).unwrap_err();
            assert!(err.contains(needle), "{err}");
        }
        call("net.clear", json!({})).unwrap();
        assert_eq!(call("net.held", json!({})).unwrap(), json!([]));
    }
}
