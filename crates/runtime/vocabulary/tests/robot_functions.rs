//! `#[robot_fn]` functions, called over the robot bridge the way a test
//! calls them: `list_functions`, `call_function {name, args}`, and for an
//! async function `poll_function {call}` until it is done.
//!
//! Own integration test (own process) because it installs a buffering
//! executor through the global first-install-wins
//! `runtime_shared::driver::install_async_executor` slot — the async case
//! needs a call that is really still running when `call_function` answers.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use runtime_macros::robot_fn;
use runtime_shared::__serde_json::{self as serde_json, json, Value};
use runtime_shared::driver::{install_async_executor, AsyncExecutor};
use runtime_vocabulary::robot::bridge::invoke_command;
use runtime_vocabulary::robot_fn::{register, unregister, RobotFunction};

thread_local! {
    static TASKS: RefCell<Vec<Pin<Box<dyn Future<Output = ()> + 'static>>>> =
        const { RefCell::new(Vec::new()) };
    static SEEDED: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    static GATE_OPEN: Cell<bool> = const { Cell::new(false) };
}

struct TestExecutor;
// SAFETY: zero-sized; all live state is thread-local (each test thread
// pumps only its own queue) — the precedent in tests/scoped_spawn.rs.
unsafe impl Send for TestExecutor {}
unsafe impl Sync for TestExecutor {}

impl AsyncExecutor for TestExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        TASKS.with(|t| t.borrow_mut().push(future));
    }
}

fn pump() {
    let mut tasks = TASKS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let mut cx = Context::from_waker(std::task::Waker::noop());
    tasks.retain_mut(|f| f.as_mut().poll(&mut cx).is_pending());
    TASKS.with(|t| t.borrow_mut().extend(tasks));
}

/// Pending until the test opens it.
struct Gate;
impl Future for Gate {
    type Output = ();
    fn poll(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if GATE_OPEN.with(Cell::get) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// Fill the inbox with `count` sample messages titled `prefix`.
#[robot_fn(name = "demo.seed")]
fn seed(count: u32, prefix: Option<String>) -> Vec<String> {
    let prefix = prefix.unwrap_or_else(|| "message".into());
    let made: Vec<String> = (1..=count).map(|i| format!("{prefix} {i}")).collect();
    SEEDED.with(|s| s.borrow_mut().extend(made.clone()));
    made
}

/// Refuses a negative balance.
#[robot_fn]
fn set_balance(cents: i64) -> Result<i64, String> {
    if cents < 0 {
        return Err(format!("a balance can't be negative ({cents})"));
    }
    Ok(cents)
}

#[robot_fn]
fn ping_unit() {}

/// Answers once the gate opens.
#[robot_fn(name = "demo.wait")]
async fn wait_for_gate(answer: u32) -> Result<u32, String> {
    Gate.await;
    if answer == 0 {
        return Err("zero is not an answer".into());
    }
    Ok(answer)
}

fn setup() {
    install_async_executor(Box::new(TestExecutor));
    register(seed::robot());
    register(set_balance::robot());
    register(ping_unit::robot());
    register(wait_for_gate::robot());
}

fn call(name: &str, args: Value) -> Result<Value, String> {
    invoke_command("call_function", &json!({ "name": name, "args": args }))
        .map(|s| serde_json::from_str(&s).expect("call_function answers JSON"))
}

fn poll(call: u64) -> Result<Value, String> {
    invoke_command("poll_function", &json!({ "call": call }))
        .map(|s| serde_json::from_str(&s).expect("poll_function answers JSON"))
}

#[test]
fn a_function_is_called_by_name_with_named_arguments() {
    setup();
    let out = call("demo.seed", json!({ "count": 2, "prefix": "hello" })).unwrap();
    assert_eq!(out, json!({ "done": true, "value": ["hello 1", "hello 2"] }));
    assert_eq!(SEEDED.with(|s| s.borrow().len()), 2, "the function itself ran");

    // An `Option` parameter may be left out.
    let out = call("demo.seed", json!({ "count": 1 })).unwrap();
    assert_eq!(out["value"], json!(["message 1"]));

    // A unit return is `null`; no `args` is an empty object.
    let out = invoke_command("call_function", &json!({ "name": "ping_unit" })).unwrap();
    assert_eq!(serde_json::from_str::<Value>(&out).unwrap(), json!({ "done": true, "value": null }));
}

#[test]
fn list_functions_reports_docs_and_signatures() {
    setup();
    let list: Value = serde_json::from_str(&invoke_command("list_functions", &json!({})).unwrap()).unwrap();
    let names: Vec<&str> = list.as_array().unwrap().iter().map(|f| f["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["demo.seed", "demo.wait", "ping_unit", "set_balance"], "sorted by name");
    let seed = &list[0];
    assert_eq!(seed["doc"], "Fill the inbox with `count` sample messages titled `prefix`.");
    assert_eq!(seed["args"], json!([{ "name": "count", "type": "u32" }, { "name": "prefix", "type": "Option<String>" }]));
    assert_eq!(seed["returns"], "Vec<String>");
    assert_eq!(seed["async"], false);
    assert_eq!(list[1]["async"], true);
}

#[test]
fn failures_say_what_went_wrong() {
    setup();
    // The function's own `Err`.
    assert_eq!(call("set_balance", json!({ "cents": -5 })).unwrap_err(), "a balance can't be negative (-5)");
    assert_eq!(call("set_balance", json!({ "cents": 5 })).unwrap()["value"], 5);
    // An argument that doesn't decode names the function and the argument.
    let err = call("set_balance", json!({ "cents": "five" })).unwrap_err();
    assert!(err.starts_with("set_balance: argument 'cents':"), "{err}");
    // A required argument left out.
    let err = call("set_balance", json!({})).unwrap_err();
    assert!(err.contains("argument 'cents'"), "{err}");
    // An unknown function lists the registered ones.
    let err = call("nope", json!({})).unwrap_err();
    assert!(err.contains("no function named 'nope'") && err.contains("demo.seed"), "{err}");
    assert!(invoke_command("call_function", &json!({})).unwrap_err().contains("'name'"));
}

#[test]
fn an_async_function_answers_pending_then_its_result() {
    setup();
    GATE_OPEN.with(|g| g.set(false));
    let first = call("demo.wait", json!({ "answer": 42 })).unwrap();
    assert_eq!(first["done"], false, "the call is still running: {first}");
    let id = first["call"].as_u64().expect("a call id to poll");

    pump();
    assert_eq!(poll(id).unwrap(), json!({ "done": false }));

    GATE_OPEN.with(|g| g.set(true));
    pump();
    assert_eq!(poll(id).unwrap(), json!({ "done": true, "value": 42 }));
    // Collected once.
    assert!(poll(id).unwrap_err().contains("no pending call"));
}

#[test]
fn an_async_failure_arrives_through_poll() {
    setup();
    GATE_OPEN.with(|g| g.set(false));
    let id = call("demo.wait", json!({ "answer": 0 })).unwrap()["call"].as_u64().unwrap();
    GATE_OPEN.with(|g| g.set(true));
    pump();
    assert_eq!(poll(id).unwrap_err(), "zero is not an answer");
    assert!(poll(id).is_err(), "a failed call is collected too");
}

#[test]
fn registering_again_replaces_and_unregister_removes() {
    setup();
    register(RobotFunction {
        name: "set_balance",
        doc: "",
        args: &[],
        returns: "()",
        is_async: false,
        call: Some(std::rc::Rc::new(|_| Ok(runtime_vocabulary::robot_fn::RobotReply::Ready(json!("replaced"))))),
    });
    assert_eq!(call("set_balance", json!({ "cents": 1 })).unwrap()["value"], "replaced");
    unregister("set_balance");
    assert!(call("set_balance", json!({})).unwrap_err().contains("no function named"));
}
