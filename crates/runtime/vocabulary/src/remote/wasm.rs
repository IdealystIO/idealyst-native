//! The bundle half over wasm: the callback table as wasm exports, for
//! `stream-host`'s `Link`. Compiled only into a remote bundle
//! (`--cfg idealyst_stream_guest`).
//!
//! Bytes cross through two buffers in this module's memory. The host asks
//! [`idealyst_ui_alloc`] for room, writes a call's arguments there, then
//! calls [`idealyst_ui_invoke`], which COPIES them out before running the
//! callback — the callback may re-enter (a getter reads a signal → kernel
//! import → host → another invoke) and reuse the buffer. A reply is written
//! to the reply buffer when the callback has finished, and the host reads
//! it immediately on return, before it calls anything else, so the nested
//! calls' replies are never in the way.

use std::cell::RefCell;

use super::bundle::{invoke, release};
use super::Cb;

thread_local! {
    static ARGS: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    static REPLY: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

/// The codec version this bundle was built with
/// ([`CODEC_VERSION`](super::CODEC_VERSION)): the app refuses to load a
/// bundle that reports a different one, or none.
#[no_mangle]
pub extern "C" fn idealyst_ui_codec_version() -> u32 {
    super::CODEC_VERSION
}

/// Room for `len` bytes of arguments; where to write them.
///
/// The buffer only grows: a call reads back just the `len` bytes the host
/// wrote (`take_args`), so one already that long is reused untouched. A
/// new one comes zeroed from the allocator (`vec!`, one `memory.fill`),
/// never from `Vec::resize`, which this size-optimized build compiles to a
/// loop storing a byte per iteration: interpreted, it made receiving a
/// list cost ~12× sending one (`tests/bundle_cost.rs` in stream-spike).
#[no_mangle]
pub extern "C" fn idealyst_ui_alloc(len: u32) -> *mut u8 {
    ARGS.with(|a| {
        let mut a = a.borrow_mut();
        if a.len() < len as usize {
            *a = vec![0u8; len as usize];
        }
        a.as_mut_ptr()
    })
}

/// The arguments the host wrote, copied out.
pub fn take_args(len: u32) -> Vec<u8> {
    ARGS.with(|a| a.borrow()[..len as usize].to_vec())
}

/// Run callback `cb` on the `len` argument bytes the host wrote; the reply,
/// packed as `ptr << 32 | len`.
#[no_mangle]
pub extern "C" fn idealyst_ui_invoke(cb: Cb, len: u32) -> i64 {
    let args = take_args(len);
    reply(invoke(cb, &args))
}

#[no_mangle]
pub extern "C" fn idealyst_ui_release(cb: Cb) {
    release(cb)
}

/// Hand `bytes` to the host as an export's result, packed as
/// `ptr << 32 | len` (what a remote component's mount export returns).
pub fn reply(bytes: Vec<u8>) -> i64 {
    REPLY.with(|r| {
        let mut r = r.borrow_mut();
        *r = bytes;
        ((r.as_ptr() as u32 as i64) << 32) | r.len() as i64
    })
}

/// Live callback entries (`bundle::live_callbacks`) — for the host's leak
/// checks.
#[no_mangle]
pub extern "C" fn idealyst_ui_live_callbacks() -> u32 {
    super::bundle::live_callbacks() as u32
}

thread_local! {
    static LAST_PANIC: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Keep the message of a panic in this bundle, for the host to read after
/// the trap it ends in (`idealyst_ui_panic_message`). A wasm panic aborts
/// with a bare `unreachable`; without this the host can only say "trapped".
/// Called by the host once, at load.
///
/// Also installs the bundle's async executor ([`executor`]).
#[no_mangle]
pub extern "C" fn idealyst_ui_init() {
    runtime_shared::driver::install_async_executor(Box::new(executor::BundleExecutor));
    std::panic::set_hook(Box::new(|info| {
        let msg = match (info.payload().downcast_ref::<&str>(), info.payload().downcast_ref::<String>()) {
            (Some(s), _) => (*s).to_string(),
            (_, Some(s)) => s.clone(),
            _ => "panic".to_string(),
        };
        let at = info.location().map(|l| format!(" ({}:{})", l.file(), l.line())).unwrap_or_default();
        let _ = LAST_PANIC.try_with(|p| *p.borrow_mut() = Some(format!("{msg}{at}")));
    }));
}

/// The last panic's message, packed like a reply; length 0 for none.
#[no_mangle]
pub extern "C" fn idealyst_ui_panic_message() -> i64 {
    reply(LAST_PANIC.with(|p| p.borrow_mut().take()).unwrap_or_default().into_bytes())
}

/// The bundle's async executor, behind runtime-shared's `spawn_async` (so
/// `spawn_then` works in a bundle as natively). A bundle has no event loop
/// of its own: a task makes progress only when something wakes it, and in
/// a bundle the only wakers that fire are host-function results arriving
/// (`bundle::HostFuture`), which run inside a call from the app. So a wake
/// polls the task right there, on the app's call — through a ready queue,
/// so a task woken while tasks are running is polled after them rather
/// than re-entrantly. Single-threaded: tasks live in a thread-local map,
/// and a waker carries only the task's id (which is what makes it `Send`).
mod executor {
    use std::cell::{Cell, RefCell};
    use std::collections::{HashMap, VecDeque};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Wake, Waker};

    type Task = Pin<Box<dyn Future<Output = ()>>>;

    thread_local! {
        static TASKS: RefCell<HashMap<u64, Task>> = RefCell::new(HashMap::new());
        static READY: RefCell<VecDeque<u64>> = const { RefCell::new(VecDeque::new()) };
        static RUNNING: Cell<bool> = const { Cell::new(false) };
        static NEXT: Cell<u64> = const { Cell::new(0) };
    }

    pub(super) struct BundleExecutor;

    impl runtime_shared::driver::AsyncExecutor for BundleExecutor {
        fn spawn(&self, future: Task) {
            let id = NEXT.with(|n| {
                n.set(n.get() + 1);
                n.get()
            });
            TASKS.with(|t| t.borrow_mut().insert(id, future));
            schedule(id);
        }
    }

    struct TaskWaker(u64);

    impl Wake for TaskWaker {
        fn wake(self: Arc<Self>) {
            schedule(self.0);
        }
    }

    fn schedule(id: u64) {
        READY.with(|r| r.borrow_mut().push_back(id));
        if RUNNING.with(|r| r.replace(true)) {
            return; // the loop below, further up the stack, will get to it
        }
        while let Some(id) = READY.with(|r| r.borrow_mut().pop_front()) {
            // Out of the map while it runs: a task that wakes itself is
            // queued, not polled re-entrantly.
            let Some(mut task) = TASKS.with(|t| t.borrow_mut().remove(&id)) else { continue };
            let waker = Waker::from(Arc::new(TaskWaker(id)));
            if task.as_mut().poll(&mut Context::from_waker(&waker)).is_pending() {
                TASKS.with(|t| t.borrow_mut().insert(id, task));
            }
        }
        RUNNING.with(|r| r.set(false));
    }
}
