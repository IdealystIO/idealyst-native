//! Canned replies for `#[server]` calls, for running client code with no
//! server: a docs preview (`#[component(preview_setup = …)]`), a component
//! test, a design-time story.
//!
//! ```ignore
//! #[server]
//! pub async fn get_user(id: u32) -> Result<User, ServerError> { … }
//!
//! server::mock::on("get_user", |(id,): (u32,)| -> Result<User, ServerError> {
//!     Ok(User { id, name: "Ada".into() })
//! });
//! server::mock::pending("slow_report"); // stays loading
//! ```
//!
//! A mock answers in place of the network: the client stub's future
//! resolves with the handler's value, so everything above it (`resource`,
//! `mutation`, the component's loading and error branches) runs as it
//! does against a real server. Returning `Err(ServerError::…)` from the
//! handler is how the error branch is shown.
//!
//! Mocks are keyed by the function's wire path — its name, unless
//! `#[server(path = "…")]` says otherwise — and are process-wide: a call
//! is usually made from an async task, which has no reactive scope to
//! look a mock up in. Installing one for a path replaces the last.
//!
//! The handler takes the wire arguments as a tuple and returns the
//! function's return type. Both are checked when a call arrives, not when
//! the mock is installed: a mismatch resolves the call with
//! `ServerError::Codec` naming the path.
//!
//! Only `#[server]` calls are answered. Channels, subscriptions and SSE
//! streams still open real connections.
//!
//! In a build with the `server` feature a `#[server]` fn is called
//! in-process and never reaches the client stub, so no mock applies.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

type Handler = Arc<dyn Fn(Value) -> Result<Value, String> + Send + Sync>;

#[derive(Clone)]
pub(crate) enum Mock {
    Reply(Handler),
    Pending,
}

/// Set once any mock is installed, so a client that never mocks pays one
/// relaxed load per call and never touches the lock.
static ANY: AtomicBool = AtomicBool::new(false);
static MOCKS: OnceLock<RwLock<HashMap<String, Mock>>> = OnceLock::new();

fn mocks() -> &'static RwLock<HashMap<String, Mock>> {
    MOCKS.get_or_init(Default::default)
}

fn install(path: &str, mock: Mock) {
    mocks().write().unwrap_or_else(|e| e.into_inner()).insert(path.to_string(), mock);
    ANY.store(true, Ordering::Relaxed);
}

/// Answer calls to the `#[server]` fn at `path` with `handler`.
///
/// `Args` is the tuple of the function's wire parameters (`()` for none,
/// `(u32,)` for one); `Ret` is its return type.
pub fn on<Args, Ret, F>(path: &str, handler: F)
where
    Args: DeserializeOwned,
    Ret: Serialize,
    F: Fn(Args) -> Ret + Send + Sync + 'static,
{
    let handler: Handler = Arc::new(move |args| {
        let args = serde_json::from_value::<Args>(args).map_err(|e| format!("arguments: {e}"))?;
        serde_json::to_value(handler(args)).map_err(|e| format!("reply: {e}"))
    });
    install(path, Mock::Reply(handler));
}

/// Leave calls to the `#[server]` fn at `path` unanswered, so whatever
/// awaits them stays in its loading state.
pub fn pending(path: &str) {
    install(path, Mock::Pending);
}

/// Remove the mock for `path`; calls go to the network again.
pub fn remove(path: &str) {
    mocks().write().unwrap_or_else(|e| e.into_inner()).remove(path);
}

/// Remove every mock.
pub fn clear() {
    mocks().write().unwrap_or_else(|e| e.into_inner()).clear();
}

pub(crate) fn lookup(path: &str) -> Option<Mock> {
    if !ANY.load(Ordering::Relaxed) {
        return None;
    }
    mocks().read().unwrap_or_else(|e| e.into_inner()).get(path).cloned()
}
