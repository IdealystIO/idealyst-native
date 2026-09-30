//! Futures without wasm-bindgen-futures: a single-threaded executor driven
//! by JS microtasks, a microtask hook, and [`JsFuture`] (a Promise as a
//! Rust future).
//!
//! One exported entry point, [`__glue_microtask`], drains two FIFO queues:
//! plain microtask callbacks ([`queue_microtask`]) and woken tasks
//! ([`spawn_local`]). Anything that needs draining asks for ONE JS
//! `queueMicrotask` (`G.queueMicrotask()`) unless one is already pending,
//! so a burst of wakes costs one crossing. Work queued while draining is
//! drained in the same pass, before control returns to the JS event loop
//! — the same "run to empty" shape as `wasm_bindgen_futures`' queue, and
//! the same ordering the framework's scheduler gets today from
//! `Promise.resolve().then(..)`.
//!
//! Single-threaded by construction (wasm32 without atomics): wakers hold
//! `Rc`s. A waker sent to another thread is a bug here exactly as it is in
//! `wasm_bindgen_futures`.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use crate::{callback::Closure, ffi, JsError, JsValue};

struct Task {
    future: RefCell<Option<Pin<Box<dyn Future<Output = ()>>>>>,
    queued: Cell<bool>,
}

#[derive(Default)]
struct Queues {
    micro: RefCell<VecDeque<Box<dyn FnOnce()>>>,
    tasks: RefCell<VecDeque<Rc<Task>>>,
    scheduled: Cell<bool>,
    draining: Cell<bool>,
}

thread_local! {
    static QUEUES: Queues = Queues::default();
}

fn request_drain() {
    QUEUES.with(|q| {
        // While draining, the drain loop will pick the new work up itself.
        if !q.scheduled.get() && !q.draining.get() {
            q.scheduled.set(true);
            unsafe { ffi::queue_microtask() }
        }
    });
}

/// Run `f` on a JS microtask (after the current task, before rendering).
pub fn queue_microtask(f: impl FnOnce() + 'static) {
    QUEUES.with(|q| q.micro.borrow_mut().push_back(Box::new(f)));
    request_drain();
}

/// Run a future to completion on the JS event loop. First polled on a
/// microtask, never synchronously inside this call.
pub fn spawn_local(future: impl Future<Output = ()> + 'static) {
    let task = Rc::new(Task {
        future: RefCell::new(Some(Box::pin(future))),
        queued: Cell::new(true),
    });
    QUEUES.with(|q| q.tasks.borrow_mut().push_back(task));
    request_drain();
}

/// Tasks spawned and not yet completed, plus queued microtask callbacks —
/// test accounting.
pub fn pending_work() -> usize {
    QUEUES.with(|q| q.micro.borrow().len() + q.tasks.borrow().len())
}

/// Drain both queues. Exported; the JS microtask calls it.
#[unsafe(no_mangle)]
pub extern "C" fn __glue_microtask() {
    QUEUES.with(|q| {
        q.scheduled.set(false);
        q.draining.set(true);
    });
    loop {
        let micro = QUEUES.with(|q| q.micro.borrow_mut().pop_front());
        if let Some(f) = micro {
            f();
            continue;
        }
        let task = QUEUES.with(|q| q.tasks.borrow_mut().pop_front());
        match task {
            Some(task) => poll_task(task),
            None => break,
        }
    }
    QUEUES.with(|q| q.draining.set(false));
}

fn poll_task(task: Rc<Task>) {
    task.queued.set(false);
    // Taken out for the poll so a wake from inside it (or a drop of the
    // last waker) never meets a borrowed cell.
    let Some(mut fut) = task.future.borrow_mut().take() else { return };
    let waker = waker_for(task.clone());
    let mut cx = Context::from_waker(&waker);
    if fut.as_mut().poll(&mut cx).is_pending() {
        *task.future.borrow_mut() = Some(fut);
    }
}

fn wake_task(task: &Rc<Task>) {
    if task.queued.replace(true) {
        return;
    }
    QUEUES.with(|q| q.tasks.borrow_mut().push_back(task.clone()));
    request_drain();
}

fn waker_for(task: Rc<Task>) -> Waker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop_raw);
    unsafe fn clone(p: *const ()) -> RawWaker {
        unsafe { Rc::increment_strong_count(p as *const Task) };
        RawWaker::new(p, &VTABLE)
    }
    unsafe fn wake(p: *const ()) {
        let task = unsafe { Rc::from_raw(p as *const Task) };
        wake_task(&task);
    }
    unsafe fn wake_by_ref(p: *const ()) {
        let task = std::mem::ManuallyDrop::new(unsafe { Rc::from_raw(p as *const Task) });
        wake_task(&task);
    }
    unsafe fn drop_raw(p: *const ()) {
        drop(unsafe { Rc::from_raw(p as *const Task) });
    }
    let raw = RawWaker::new(Rc::into_raw(task) as *const (), &VTABLE);
    // SAFETY: the vtable upholds the RawWaker contract for an `Rc<Task>`
    // on one thread (see the module docs on single-threadedness).
    unsafe { Waker::from_raw(raw) }
}

struct Settle {
    result: Option<Result<JsValue, JsValue>>,
    waker: Option<Waker>,
}

/// A JS Promise as a Rust future. Resolves to the fulfilled value, or
/// `Err` with the rejection reason.
///
/// Dropping it before the promise settles is fine: its reactions are
/// minted SILENT, so the eventual settle is a no-op rather than a
/// "callback called after drop" error.
pub struct JsFuture {
    state: Rc<RefCell<Settle>>,
    _reactions: (Closure, Closure),
}

impl JsFuture {
    /// `promise.then(ok, err)`, awaited. Anything thenable works; a
    /// non-thenable is an immediate `Err` (the `.then` call throws).
    pub fn new(promise: &JsValue) -> JsFuture {
        let state = Rc::new(RefCell::new(Settle { result: None, waker: None }));
        let settle = |state: Rc<RefCell<Settle>>, ok: bool| {
            move |v: JsValue| {
                let waker = {
                    let mut s = state.borrow_mut();
                    s.result = Some(if ok { Ok(v) } else { Err(v) });
                    s.waker.take()
                };
                if let Some(w) = waker {
                    w.wake();
                }
            }
        };
        let ok = Closure::once_silent(settle(state.clone(), true));
        let err = Closure::once_silent(settle(state.clone(), false));
        unsafe { ffi::promise_then(promise.raw(), ok.as_js().raw(), err.as_js().raw()) };
        JsFuture { state, _reactions: (ok, err) }
    }

    /// `Promise.resolve(value)`, awaited — always settles on a microtask.
    pub fn resolve(value: &JsValue) -> JsFuture {
        let p = unsafe { JsValue::from_raw(ffi::promise_resolve(value.raw())) };
        JsFuture::new(&p)
    }
}

impl From<JsValue> for JsFuture {
    fn from(promise: JsValue) -> JsFuture {
        JsFuture::new(&promise)
    }
}

impl Future for JsFuture {
    type Output = Result<JsValue, JsError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut s = self.state.borrow_mut();
        match s.result.take() {
            Some(Ok(v)) => Poll::Ready(Ok(v)),
            Some(Err(e)) => Poll::Ready(Err(JsError::from(e))),
            None => {
                s.waker = Some(cx.waker().clone());
                Poll::Pending
            }
        }
    }
}
