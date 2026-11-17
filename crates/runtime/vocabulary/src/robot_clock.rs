//! The robot's `clock.*` functions: a test sets, freezes, advances or
//! re-zones the app's wall clock, so time-dependent rules (a late check-in,
//! a shift rolling past midnight, a stale open shift) can be driven to the
//! moment they apply instead of waited for.
//!
//! They set the `test-clock` fake, which both wall clocks report while it
//! is set: the framework's (`runtime_shared::time::epoch_millis` /
//! `local_offset_minutes`, a date picker's "today") and the `datetime`
//! SDK's `SystemClock` (`now_utc`, `now_chrono_utc`, what app code reads).
//! The monotonic clock and timers keep real time, and a server the app
//! talks to keeps its own clock.
//!
//! Framework functions, not an SDK's: registered by the robot itself the
//! first time a test lists or calls a function, so every robot build has
//! them with no `register_robot_controls()` call.

use runtime_shared::__serde_json::{self as serde_json, json, Value};
use runtime_shared::time;
use std::rc::Rc;

use crate::robot_fn::{register, RobotArg, RobotFunction, RobotReply};

/// The real instant in µs, past the fake: what the fake is anchored to.
fn real_micros() -> i64 {
    time::real_epoch_millis().saturating_mul(1000)
}

const MS: i64 = 1000;

fn now() -> Value {
    json!({ "epoch_ms": time::epoch_millis(), "offset_minutes": time::local_offset_minutes() })
}

fn arg<T: serde::de::DeserializeOwned>(args: &Value, function: &str, name: &str) -> Result<T, String> {
    crate::robot_fn::__arg(args, function, name)
}

fn function(
    name: &'static str,
    doc: &'static str,
    args: &'static [RobotArg],
    call: impl Fn(&Value) -> Result<Value, String> + 'static,
) -> RobotFunction {
    RobotFunction {
        name,
        doc,
        args,
        returns: "ClockNow",
        is_async: false,
        call: Some(Rc::new(move |a: &Value| call(a).map(RobotReply::Ready))),
    }
}

/// Register the `clock.*` functions (idempotent: re-registering replaces).
pub(crate) fn register_all() {
    register(function(
        "clock.set",
        "Make the wall clock read `epoch_ms` (milliseconds since 1970 UTC) now. It runs on from \
         there unless `frozen` is true. Returns the clock's reading {epoch_ms, offset_minutes}.",
        &[RobotArg { name: "epoch_ms", ty: "i64" }, RobotArg { name: "frozen", ty: "Option<bool>" }],
        |a| {
            let at: i64 = arg(a, "clock.set", "epoch_ms")?;
            let frozen: Option<bool> = arg(a, "clock.set", "frozen")?;
            test_clock::set(at.saturating_mul(MS), real_micros(), frozen.unwrap_or(false));
            Ok(now())
        },
    ));
    register(function(
        "clock.advance",
        "Move the wall clock `ms` milliseconds forward (negative: back), from the faked time if one \
         is set, else from the real time. Timers are not fired early: they keep real time.",
        &[RobotArg { name: "ms", ty: "i64" }],
        |a| {
            let ms: i64 = arg(a, "clock.advance", "ms")?;
            test_clock::advance(ms.saturating_mul(MS), real_micros());
            Ok(now())
        },
    ));
    register(function(
        "clock.freeze",
        "Stop the wall clock where it reads now (`frozen: false` lets it run again).",
        &[RobotArg { name: "frozen", ty: "Option<bool>" }],
        |a| {
            let frozen: Option<bool> = arg(a, "clock.freeze", "frozen")?;
            test_clock::freeze(frozen.unwrap_or(true), real_micros());
            Ok(now())
        },
    ));
    register(function(
        "clock.zone",
        "Report `offset_minutes` (minutes to add to UTC, e.g. -420 for UTC-7) as the device's local \
         zone; omit it for the real zone again.",
        &[RobotArg { name: "offset_minutes", ty: "Option<i32>" }],
        |a| {
            let offset: Option<i32> = arg(a, "clock.zone", "offset_minutes")?;
            test_clock::set_zone(offset.map(|m| m * 60));
            Ok(now())
        },
    ));
    register(function("clock.clear", "Back to the real wall clock and zone.", &[], |_| {
        test_clock::clear();
        Ok(now())
    }));
    register(function("clock.now", "The wall clock's reading: {epoch_ms, offset_minutes}.", &[], |_| {
        Ok(now())
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::robot::bridge::invoke_command;

    fn call(name: &str, args: Value) -> Result<Value, String> {
        let reply = invoke_command("call_function", &json!({ "name": name, "args": args }))?;
        let v: Value = serde_json::from_str(&reply).map_err(|e| e.to_string())?;
        Ok(v["value"].clone())
    }

    /// want_0a1bd568 (CrewForge): the kiosk's time rules (a late check-in
    /// needs a reason, a night shift rolls over) could only be tested at
    /// the hour they apply. A test now sets the clock without registering
    /// anything: the functions are the robot's own.
    #[test]
    fn a_test_sets_freezes_advances_and_rezones_the_wall_clock() {
        // 2026-11-17T22:30:00Z
        let at = 1_794_954_600_000_i64;
        let set = call("clock.set", json!({ "epoch_ms": at, "frozen": true })).unwrap();
        assert_eq!(set["epoch_ms"], at);
        assert_eq!(time::epoch_millis(), at, "what the datetime SDK reads");

        let later = call("clock.advance", json!({ "ms": 2 * 60 * 60 * 1000 })).unwrap();
        assert_eq!(later["epoch_ms"], at + 7_200_000, "two hours on: past midnight");

        call("clock.zone", json!({ "offset_minutes": -420 })).unwrap();
        assert_eq!(time::local_offset_minutes(), -420);

        let err = call("clock.set", json!({ "epoch_ms": "soon" })).unwrap_err();
        assert!(err.contains("epoch_ms"), "{err}");

        call("clock.clear", json!({})).unwrap();
        assert_ne!(time::epoch_millis(), at + 7_200_000, "the real clock again");
        let listed = invoke_command("list_functions", &json!({})).unwrap();
        for name in ["clock.set", "clock.advance", "clock.freeze", "clock.zone", "clock.clear", "clock.now"] {
            assert!(listed.contains(name), "{name} is listed: {listed}");
        }
    }
}
