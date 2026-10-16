//! Functions a test can call by name over the robot bridge — app or SDK
//! code with no element to act on: seed demo data, set a fake location,
//! take the device offline, open a deep link, read what notifications were
//! posted.
//!
//! Declared with `#[robot_fn]` on a free function, which keeps the function
//! as written and adds `<name>::robot()`, the [`RobotFunction`] to
//! [`register`]:
//!
//! ```ignore
//! /// Fill the inbox with `count` sample messages.
//! #[robot_fn]
//! fn seed_inbox(count: u32) -> Result<(), String> { … }
//!
//! // At boot, on the UI thread:
//! robot_fn::register(seed_inbox::robot());
//! ```
//!
//! A test calls it with `call_function {name, args}`, the arguments an
//! object keyed by parameter name (each decoded with `serde`); the return
//! value comes back as JSON. A `Result`'s `Err` fails the call with its
//! `Display`. `list_functions` lists what is registered, with each
//! function's doc comment and signature.
//!
//! # Async functions
//!
//! An `async fn` is spawned on the backend's executor
//! (`runtime_shared::driver::spawn_async`). `call_function` answers
//! `{"done": false, "call": N}` and the client asks `poll_function {call}`
//! until it answers `{"done": true, "value": …}`. The reply never blocks
//! the bridge: every transport (TCP, relay, web) answers each request
//! within its own timeout, and a function may take longer than that.
//!
//! # Where it runs
//!
//! On the UI thread, outside any `World` — like an event handler, so it
//! may read and write signals but not create them (see
//! `scoped_spawn`) — and the robot settles staged reactive work after a
//! synchronous call returns, as it does after every action.
//!
//! # Builds
//!
//! Always compiled, so registration needs no `cfg`. Without the
//! vocabulary's `robot` feature `#[robot_fn]` builds no adapter (the
//! function's arguments are never decoded, so nothing of it reaches the
//! binary), and [`register`] drops what it is given.
//!
//! # SDK test controls
//!
//! An SDK fakes what a test can't make the device do — a location fix, a
//! network drop — with robot functions under a namespaced name
//! (`location.set`), registered by its `register_robot_controls()`. The
//! fake and its hooks compile only into robot builds ([`robot_only!`],
//! [`robot_select!`]), so a release build carries none of it.

use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;

use runtime_shared::__serde_json as serde_json;

/// One parameter of a [`RobotFunction`]: its name and its Rust type as
/// written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RobotArg {
    pub name: &'static str,
    pub ty: &'static str,
}

/// A future an async robot function returns: its JSON result, or the
/// message it failed with.
pub type RobotFuture = Pin<Box<dyn Future<Output = Result<serde_json::Value, String>>>>;

/// What calling a robot function produced.
pub enum RobotReply {
    /// A synchronous function's result.
    Ready(serde_json::Value),
    /// An async function's future, for the robot to drive.
    Pending(RobotFuture),
}

/// Decodes the arguments and calls the function. `Err` is a bad argument or
/// a synchronous function's failure.
pub type RobotCall = Rc<dyn Fn(&serde_json::Value) -> Result<RobotReply, String>>;

/// A function registered with the robot. `#[robot_fn]` builds one as
/// `<name>::robot()`; an SDK whose function is a closure over live state
/// may build one by hand.
pub struct RobotFunction {
    /// The name a test calls it by. `#[robot_fn]` uses the function's name
    /// unless given `name = "…"` (SDKs namespace theirs: `location.set`).
    pub name: &'static str,
    /// Its doc comment.
    pub doc: &'static str,
    pub args: &'static [RobotArg],
    /// The Rust return type as written (`()` when none).
    pub returns: &'static str,
    pub is_async: bool,
    /// `None` in a build without the `robot` feature.
    pub call: Option<RobotCall>,
}

/// Register `function` under its name, replacing one registered before
/// under the same name. Call on the UI thread (the registry is
/// thread-local, like the robot's element registry). A no-op without the
/// `robot` feature.
#[allow(unused_variables)]
pub fn register(function: RobotFunction) {
    #[cfg(feature = "robot")]
    real::register(function);
}

/// Remove the function registered under `name`, if any.
#[allow(unused_variables)]
pub fn unregister(name: &str) {
    #[cfg(feature = "robot")]
    real::unregister(name);
}

#[cfg(feature = "robot")]
#[doc(hidden)]
/// Wrap a function's return value as the JSON a test receives. What
/// `#[robot_fn]` emits for a value that is not a `Result`.
pub fn __ok_json<T: serde::Serialize>(value: T) -> Result<serde_json::Value, String> {
    serde_json::to_value(value).map_err(|e| format!("the return value does not serialize: {e}"))
}

#[cfg(feature = "robot")]
#[doc(hidden)]
/// What `#[robot_fn]` emits for a `Result`: `Ok` as JSON, `Err` as the
/// failure's `Display`.
pub fn __result_json<T: serde::Serialize, E: std::fmt::Display>(
    value: Result<T, E>,
) -> Result<serde_json::Value, String> {
    match value {
        Ok(v) => __ok_json(v),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(feature = "robot")]
#[doc(hidden)]
/// Decode the argument `name` of the function `function` from the call's
/// argument object. A missing argument decodes from `null`, so an
/// `Option` parameter may be left out.
pub fn __arg<T: serde::de::DeserializeOwned>(
    args: &serde_json::Value,
    function: &str,
    name: &str,
) -> Result<T, String> {
    let value = args.get(name).cloned().unwrap_or(serde_json::Value::Null);
    serde_json::from_value(value).map_err(|e| format!("{function}: argument '{name}': {e}"))
}

/// Pick tokens by build: the `on` tokens with the vocabulary's `robot`
/// feature, the `off` tokens without it. For an SDK whose test controls
/// need a stand-in in release builds (a guard field's type, a hook that
/// passes its value through), and for what `#[robot_fn]` emits. The
/// feature is the vocabulary's, so the crate using it declares no cfg of
/// its own.
///
/// ```ignore
/// runtime_vocabulary::robot_select! {
///     on: { mod controls; use controls::Tap; }
///     off: { type Tap = (); }
/// }
/// ```
#[cfg(feature = "robot")]
#[macro_export]
macro_rules! robot_select {
    (on: { $($on:tt)* } off: { $($off:tt)* }) => { $($on)* };
}
/// Pick tokens by build: the `on` tokens with the vocabulary's `robot`
/// feature, the `off` tokens without it. See the `robot_fn` module.
#[cfg(not(feature = "robot"))]
#[macro_export]
macro_rules! robot_select {
    (on: { $($on:tt)* } off: { $($off:tt)* }) => { $($off)* };
}

/// Code an SDK compiles only into robot builds — its test controls and the
/// state they fake: `robot_select!` with nothing for `off`.
///
/// ```ignore
/// runtime_vocabulary::robot_only! { mod controls; }
/// ```
#[cfg(feature = "robot")]
#[macro_export]
macro_rules! robot_only {
    ($($t:tt)*) => { $($t)* };
}
/// Code an SDK compiles only into robot builds — its test controls and the
/// state they fake: `robot_select!` with nothing for `off`.
#[cfg(not(feature = "robot"))]
#[macro_export]
macro_rules! robot_only {
    ($($t:tt)*) => {};
}

#[cfg(feature = "robot")]
pub(crate) use real::{call_json, list_json, poll_json};

#[cfg(feature = "robot")]
mod real {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::{BTreeMap, HashMap};

    type Outcome = Rc<RefCell<Option<Result<serde_json::Value, String>>>>;

    runtime_tls::thread_local! {
        static FUNCTIONS: RefCell<BTreeMap<&'static str, RobotFunction>> =
            const { RefCell::new(BTreeMap::new()) };
        /// Async calls not yet collected by `poll_function`: their result
        /// slot, filled when the future completes.
        static PENDING: RefCell<HashMap<u64, Outcome>> = RefCell::new(HashMap::new());
        static NEXT_CALL: Cell<u64> = const { Cell::new(1) };
    }

    pub(super) fn register(function: RobotFunction) {
        FUNCTIONS.with(|f| f.borrow_mut().insert(function.name, function));
    }

    pub(super) fn unregister(name: &str) {
        FUNCTIONS.with(|f| f.borrow_mut().remove(name));
    }

    /// `list_functions`: every registered function, by name.
    pub(crate) fn list_json() -> String {
        let items: Vec<serde_json::Value> = FUNCTIONS.with(|f| {
            f.borrow()
                .values()
                .map(|fun| {
                    serde_json::json!({
                        "name": fun.name,
                        "doc": fun.doc,
                        "args": fun.args.iter().map(|a| serde_json::json!({"name": a.name, "type": a.ty})).collect::<Vec<_>>(),
                        "returns": fun.returns,
                        "async": fun.is_async,
                    })
                })
                .collect()
        });
        serde_json::Value::Array(items).to_string()
    }

    fn done(value: serde_json::Value) -> String {
        serde_json::json!({"done": true, "value": value}).to_string()
    }

    /// `call_function {name, args}`.
    pub(crate) fn call_json(name: &str, args: &serde_json::Value) -> Result<String, String> {
        // Cloned out under a short borrow: the function may register or
        // unregister functions itself.
        let call = FUNCTIONS.with(|f| {
            let f = f.borrow();
            match f.get(name) {
                Some(fun) => fun
                    .call
                    .clone()
                    .ok_or_else(|| format!("function '{name}' was registered without its adapter")),
                None => Err(format!(
                    "no function named '{name}'; registered: [{}]",
                    f.keys().copied().collect::<Vec<_>>().join(", ")
                )),
            }
        })?;
        let reply = call(args);
        crate::robot::settle();
        match reply? {
            RobotReply::Ready(value) => Ok(done(value)),
            RobotReply::Pending(future) => {
                let slot: Outcome = Rc::new(RefCell::new(None));
                let fill = slot.clone();
                runtime_shared::driver::spawn_async(async move {
                    let out = future.await;
                    *fill.borrow_mut() = Some(out);
                    crate::robot::settle();
                });
                // Without an installed executor native `spawn_async` runs
                // the future to completion before returning: answer at once.
                if let Some(out) = slot.borrow_mut().take() {
                    return out.map(done);
                }
                let id = NEXT_CALL.with(|n| {
                    let id = n.get();
                    n.set(id + 1);
                    id
                });
                PENDING.with(|p| p.borrow_mut().insert(id, slot));
                Ok(serde_json::json!({"done": false, "call": id}).to_string())
            }
        }
    }

    /// `poll_function {call}`: `{"done": false}` while the call runs; its
    /// result (or failure) once, after which the call is forgotten.
    pub(crate) fn poll_json(call: u64) -> Result<String, String> {
        let slot = PENDING
            .with(|p| p.borrow().get(&call).cloned())
            .ok_or_else(|| format!("no pending call {call} (already collected, or never started)"))?;
        let out = slot.borrow_mut().take();
        match out {
            None => Ok(r#"{"done":false}"#.into()),
            Some(out) => {
                PENDING.with(|p| p.borrow_mut().remove(&call));
                out.map(done)
            }
        }
    }
}
