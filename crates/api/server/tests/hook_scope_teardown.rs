//! `use_socket` / `use_sse` — teardown while the connection is in flight.
//!
//! # The bug these pin
//!
//! Both hooks create their `incoming`/`status`/`sender` signals in the
//! CALLING scope and then drive the connection from a fully detached
//! task (`driver::spawn_async`, which takes no liveness token). Closing
//! the connection is what makes that task resume — and closing it is
//! exactly what scope teardown does. So the ordinary unmount path woke
//! the task into `status.set(…)` against slots the dropped `Owned` had
//! already freed, aborting the module with
//! `idealyst[stale-signal-handle]`.
//!
//! On web it fired on every page load of an app whose socket URL carries
//! a credential read from storage: the key hydrates a microtask after
//! mount, the `switch` keyed on it re-keys, the first branch's scope
//! drops, its socket closes, and the detached task resumes into a freed
//! slot. There is no app-side fix — the signals belong to a scope the
//! author does not own.
//!
//! # Why the scope is dropped BEFORE the first poll
//!
//! The test executor below is buffering and hand-pumped, so nothing in
//! the task runs until `pump()`. Dropping the scope first therefore
//! guarantees the connect resolves — and every write site is reached —
//! strictly after teardown, with no race against the real I/O thread.
//! That is the failing case, deterministically.
//!
//! Own integration test (own process) because it installs an executor
//! through the global first-install-wins
//! `runtime_core::driver::install_async_executor` slot — same isolation
//! rationale as `runtime-vocabulary/tests/scoped_spawn.rs`.

#![cfg(not(feature = "server"))]

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::task::Context;

use runtime_core::driver::{install_async_executor, AsyncExecutor};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum ServerMsg {
    Tick(u32),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum ClientMsg {
    Subscribe,
}

// ---------------------------------------------------------------------
// Buffering test executor — nothing runs until `pump()`.
// ---------------------------------------------------------------------

thread_local! {
    static TASKS: RefCell<Vec<Pin<Box<dyn Future<Output = ()> + 'static>>>> =
        const { RefCell::new(Vec::new()) };
}

struct TestExecutor;
// SAFETY: zero-sized; all live state is thread-local and each thread
// pumps only its own queue — the precedent in
// `runtime-vocabulary/tests/scoped_spawn.rs`.
unsafe impl Send for TestExecutor {}
unsafe impl Sync for TestExecutor {}

impl AsyncExecutor for TestExecutor {
    fn spawn(&self, future: Pin<Box<dyn Future<Output = ()> + 'static>>) {
        TASKS.with(|t| t.borrow_mut().push(future));
    }
}

/// Poll every queued future once; completed ones drop.
fn pump() {
    let mut tasks = TASKS.with(|t| std::mem::take(&mut *t.borrow_mut()));
    let waker = std::task::Waker::noop();
    let mut cx = Context::from_waker(waker);
    tasks.retain_mut(|f| f.as_mut().poll(&mut cx).is_pending());
    TASKS.with(|t| {
        let mut q = t.borrow_mut();
        tasks.append(&mut q);
        *q = tasks;
    });
}

/// Pump until the queue drains. The real connect runs on the transport's
/// own I/O thread and the noop waker cannot re-poll us, so this spins
/// with a short sleep rather than waiting on a notification. Returns
/// whether the queue actually drained — a test asserts it, so a task
/// that never reaches its write sites cannot pass vacuously.
fn pump_until_idle() -> bool {
    for _ in 0..400 {
        pump();
        if TASKS.with(|t| t.borrow().is_empty()) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    false
}

/// Both hooks point at loopback port 1, which refuses immediately, so
/// the connect resolves `Err` without a server — and `Err` is one of the
/// four post-teardown write sites (`status.set(Error)`).
const REFUSED_WS: &str = "ws://127.0.0.1:1";
const REFUSED_SSE: &str = "http://127.0.0.1:1/events";

// ---------------------------------------------------------------------

/// Regression: the detached recv task wrote `status` after the scope
/// that owns it was torn down — `idealyst[stale-signal-handle]`, which
/// on web aborts the wasm module.
#[test]
fn regression_use_socket_teardown_does_not_write_freed_slots() {
    install_async_executor(Box::new(TestExecutor));
    runtime_core::__with_fresh_world(|| {
        let (_handle, owned) = runtime_world::collect_owned(|| {
            server::use_socket::<ServerMsg, ClientMsg>(REFUSED_WS)
        });

        // Teardown BEFORE the task has been polled once: every write the
        // task can make is now a write into a freed slot.
        drop(owned);

        assert!(
            pump_until_idle(),
            "the connect must resolve — otherwise the write sites are never reached \
             and this test proves nothing"
        );
        // Reaching here at all is the assertion: an unguarded write
        // panics inside `pump`.
    });
}

/// The `use_sse` half of the same defect — same detached-task shape,
/// same four write sites.
#[test]
fn regression_use_sse_teardown_does_not_write_freed_slots() {
    install_async_executor(Box::new(TestExecutor));
    runtime_core::__with_fresh_world(|| {
        let (_handle, owned) =
            runtime_world::collect_owned(|| server::use_sse::<ServerMsg>(REFUSED_SSE));

        drop(owned);

        assert!(
            pump_until_idle(),
            "the connect must resolve — otherwise the write sites are never reached \
             and this test proves nothing"
        );
    });
}

/// The inverse, so a "treat every socket write as dead" fix cannot pass:
/// while the scope LIVES, the task's writes must still land. The connect
/// fails, so the observable write is `status = Error`.
#[test]
fn use_socket_still_reports_status_while_its_scope_lives() {
    install_async_executor(Box::new(TestExecutor));
    runtime_core::__with_fresh_world(|| {
        let (handle, owned) = runtime_world::collect_owned(|| {
            server::use_socket::<ServerMsg, ClientMsg>(REFUSED_WS)
        });

        assert!(pump_until_idle(), "the connect must resolve");
        // `set` stages; the driver's flush is what commits it. There is
        // no driver in a hook-only test, so commit by hand.
        runtime_core::__flush_test_world();
        assert_eq!(
            handle.status(),
            server::SocketStatus::Error,
            "a live scope must still see the connect failure"
        );
        drop(owned);
    });
}
