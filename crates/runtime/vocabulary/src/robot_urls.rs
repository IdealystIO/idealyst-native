//! The robot's `urls.*` functions: keep the app in front when it opens an
//! external URL. While `urls.capture` is on, `open_url` records the URL
//! instead of handing it to the system, and a test asserts on it with
//! `urls.opened`. A phone app that opens a URL goes to the background and
//! the OS suspends it, which ends a run; a browser tab keeps running.
//! Capturing makes every backend behave the same.
//!
//! The test harness turns capture on when it connects and clears the
//! record before each test. Registered by the robot itself, like `net.*`.

use runtime_shared::__serde_json::{json, Value};
use runtime_shared::host;

use crate::robot_fn::{register, RobotArg, RobotFunction, RobotReply};

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
        call: Some(std::rc::Rc::new(move |a: &Value| call(a).map(RobotReply::Ready))),
    }
}

pub(crate) fn register_all() {
    register(function(
        "urls.capture",
        "While `on`, `open_url` records the URL (see `urls.opened`) instead of opening it, so \
         the app stays in front. Off opens them again and drops the record. Returns {capturing}.",
        &[RobotArg { name: "on", ty: "bool" }],
        "{capturing}",
        |a| {
            let on: bool = crate::robot_fn::__arg(a, "urls.capture", "on")?;
            host::capture_url_opens(on);
            Ok(json!({ "capturing": on }))
        },
    ));
    register(function(
        "urls.opened",
        "The URLs `open_url` recorded while capturing, oldest first.",
        &[],
        "[String]",
        |_| Ok(json!(host::captured_url_opens())),
    ));
    register(function(
        "urls.clear",
        "Forget the recorded URLs; capturing stays as it was.",
        &[],
        "()",
        |_| {
            host::clear_captured_url_opens();
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

    /// A test sees what the app tried to open, and the app stays in front.
    #[test]
    fn a_test_captures_the_urls_the_app_opens() {
        let opened = std::rc::Rc::new(std::cell::Cell::new(0));
        let sink = opened.clone();
        host::install_url_opener(Some(std::rc::Rc::new(move |_: &str| sink.set(sink.get() + 1))));

        assert_eq!(call("urls.capture", json!({ "on": true })).unwrap()["capturing"], true);
        runtime_shared::open_url("http://h/a.pdf");
        assert_eq!(call("urls.opened", json!({})).unwrap(), json!(["http://h/a.pdf"]));
        assert_eq!(opened.get(), 0);
        call("urls.clear", json!({})).unwrap();
        assert_eq!(call("urls.opened", json!({})).unwrap(), json!([]));

        call("urls.capture", json!({ "on": false })).unwrap();
        runtime_shared::open_url("http://h/b");
        assert_eq!(opened.get(), 1);
        host::install_url_opener(None);
    }
}
