//! Web backend: run the job in a Web Worker that instantiates the SAME app
//! module, on the framework's own glue (`web_glue::worker`) — no
//! wasm-bindgen, no SharedArrayBuffer, so no COOP/COEP headers.
//!
//! # Dispatch
//!
//! A job crosses as two function-table indices and postcard bytes:
//!
//! * `job` — the `#[offload::job]` fn pointer itself (`fn(T) -> R`);
//! * `call` — the monomorphized [`dispatch::<T, R>`] that decodes `T`,
//!   calls `job`, and encodes `R`.
//!
//! Both are plain `fn` pointers, i.e. indices into the module's function
//! table, which every instance of the same module bytes shares (see
//! `web_glue::worker` for why, and for the two cases — a wasm-split chunk
//! the worker never loaded, a function a dev hot patch added — where an
//! index has no twin and the worker traps, which surfaces here as
//! [`OffloadError::Canceled`]). The engine type-checks `call_indirect`, so a
//! wrong index traps; it cannot run with the wrong signature.
//!
//! # Pool
//!
//! Workers start lazily, up to `navigator.hardwareConcurrency`, and each runs
//! one job at a time (a job is synchronous CPU work, so a worker could not
//! interleave two anyway); jobs beyond that queue here, in submission order.
//! A worker announces itself ready before it is given work — it is not
//! listening until its entry fn has run.
//!
//! # Failures are surfaced, never hung
//!
//! A job that panics, or a worker that traps or fails to load, resolves the
//! awaiting future with `OffloadError::Canceled` (the same variant native
//! returns when its thread dies) and logs the cause to the console:
//!
//! * a panic: the worker's panic hook posts the message first, so the log
//!   names the job and says what panicked;
//! * any other trap (`unreachable`, a stale table index, out of memory):
//!   the Worker's `error` event;
//! * a worker that never starts (module failed to load, a bundle built
//!   without worker support): its `error` event, or the spawn error.
//!
//! A worker that panicked or trapped is terminated and replaced on demand —
//! its instance's state (borrowed `RefCell`s, a half-run allocator) is not
//! trusted again.

use std::cell::RefCell;
use std::collections::VecDeque;

use futures_channel::oneshot;
use serde::{Deserialize, Serialize};
use web_glue::dom::{console, Event, Listener, ListenerOptions, MessageEvent};
use web_glue::worker::{self, Worker};
use web_glue::{JsCast, JsValue};

use crate::{Handle, OffloadError};

/// Run `handle`'s job with `arg` on a Web Worker and await the result.
///
/// `Err(OffloadError::Canceled)` when the job could not produce a result —
/// it panicked or trapped, its worker failed to start, or the argument /
/// result failed to (de)serialize. The cause is logged to the console.
pub async fn run<T, R>(handle: Handle<T, R>, arg: &T) -> Result<R, OffloadError>
where
    T: Serialize + for<'de> Deserialize<'de>,
    R: Serialize + for<'de> Deserialize<'de>,
{
    let name = handle.name;
    let bytes = postcard::to_allocvec(arg)
        .map_err(|e| fail(name, &format!("could not serialize its argument: {e}")))?;
    let call: Dispatch = dispatch::<T, R>;
    let (tx, rx) = oneshot::channel();
    let job = Job { name, call: call as usize as u32, job: handle.f as usize as u32, arg: bytes, tx };
    POOL.with(|p| p.borrow_mut().queue.push_back(job));
    pump();
    let out = rx.await.map_err(|_| OffloadError::Canceled)?;
    postcard::from_bytes(&out).map_err(|e| fail(name, &format!("could not deserialize its result: {e}")))
}

fn fail(name: &str, why: &str) -> OffloadError {
    console::error_1(&JsValue::from_str(&format!("offload: job `{name}` {why}")));
    OffloadError::Canceled
}

// ---- the worker side ---------------------------------------------------------

/// Decode `T`, call the job at table index `job`, encode `R`. Runs in the
/// worker; the parent sends `dispatch::<T, R>`'s own index as `call`.
type Dispatch = fn(usize, &[u8]) -> Result<Vec<u8>, String>;

fn dispatch<T, R>(job: usize, input: &[u8]) -> Result<Vec<u8>, String>
where
    T: for<'de> Deserialize<'de>,
    R: Serialize,
{
    // SAFETY: `job` is `handle.f as usize` from the parent's instance of
    // this module — the same `fn(T) -> R` here (shared table layout; see the
    // module docs). The engine checks the signature at the call.
    let f = unsafe { std::mem::transmute::<usize, fn(T) -> R>(job) };
    let arg: T = postcard::from_bytes(input).map_err(|e| format!("could not deserialize its argument: {e}"))?;
    postcard::to_allocvec(&f(arg)).map_err(|e| format!("could not serialize its result: {e}"))
}

// Message kinds, worker → parent (`t`).
const READY: u32 = 0;
const OK: u32 = 1;
const ERR: u32 = 2;
const PANIC: u32 = 3;

web_glue::import! {
    // Parent → worker: one job. `arg` is copied out of linear memory and
    // its buffer transferred, so the worker gets it without a second copy.
    #[catch]
    fn js_post_job(w: u32, call: u32, job: u32, p: usize, l: usize) =
        "(w, c, j, p, l) => { const a = G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0)); \
           G.get(w).postMessage({ call: c >>> 0, job: j >>> 0, arg: a }, [a.buffer]); }";
    // Worker → parent: `{ t, data }` with `data` the bytes at (p, l).
    #[catch]
    fn js_post_reply(t: u32, p: usize, l: usize) =
        "(t, p, l) => { const a = G.u8().slice(p >>> 0, (p >>> 0) + (l >>> 0)); \
           globalThis.postMessage({ t: t >>> 0, data: a }, [a.buffer]); }";
    fn js_job_field(d: u32, which: u32) -> u32 =
        "(d, w) => { const v = G.get(d); return (w === 0 ? v.call : v.job) >>> 0; }";
    fn js_job_arg(d: u32) -> u32 = "(d) => G.add(G.get(d).arg)";
    fn js_reply_kind(d: u32) -> u32 = "(d) => { const v = G.get(d); return (v && typeof v.t === 'number') ? v.t >>> 0 : 99; }";
    fn js_reply_data(d: u32) -> u32 = "(d) => G.add(G.get(d).data)";
    fn js_error_message(e: u32, out: usize) =
        "(e, o) => { const v = G.get(e); G.retStr(String(v && v.message ? v.message : v), o); }";
}

fn reply(kind: u32, bytes: &[u8]) {
    // A failure to post leaves the parent waiting on this worker; there is
    // no other channel left, so it is loud (and, in a job, a trap the
    // parent sees as the `error` event).
    unsafe { js_post_reply(kind, bytes.as_ptr() as usize, bytes.len()) }.expect("offload: postMessage to the parent failed");
}

/// The Worker's entry (`web_glue::worker::spawn`): install the panic hook
/// and the job listener, then report ready.
fn worker_main() {
    // Reports the panic text BEFORE the trap that follows it, so the parent
    // can say what went wrong (stderr is a no-op on wasm32).
    std::panic::set_hook(Box::new(|info| {
        let msg = info.to_string();
        let _ = unsafe { js_post_reply(PANIC, msg.as_ptr() as usize, msg.len()) };
    }));
    Listener::new(worker::scope(), "message", ListenerOptions::default(), |ev: Event| {
        let data = ev.unchecked_into::<MessageEvent>().data();
        let call = unsafe { js_job_field(data.raw(), 0) };
        let job = unsafe { js_job_field(data.raw(), 1) };
        let arg: web_glue::js::Uint8Array =
            unsafe { JsValue::from_raw(js_job_arg(data.raw())) }.unchecked_into();
        let input = arg.to_vec();
        // SAFETY: `call` is `dispatch::<T, R> as Dispatch` from the parent's
        // instance of this module (see the module docs).
        let call = unsafe { std::mem::transmute::<usize, Dispatch>(call as usize) };
        match call(job as usize, &input) {
            Ok(out) => reply(OK, &out),
            Err(why) => reply(ERR, why.as_bytes()),
        }
    })
    // Lives as long as the worker's global scope, i.e. the worker.
    .into_target_owned();
    reply(READY, &[]);
}

// ---- the parent side ---------------------------------------------------------

struct Job {
    name: &'static str,
    call: u32,
    job: u32,
    arg: Vec<u8>,
    tx: oneshot::Sender<Vec<u8>>,
}

/// What a running job leaves in its slot: enough to answer and to log.
struct Running {
    name: &'static str,
    tx: oneshot::Sender<Vec<u8>>,
}

enum State {
    Starting,
    Idle,
    Busy(Running),
}

struct Slot {
    id: u32,
    worker: Worker,
    state: State,
    _listeners: [Listener; 2],
}

impl Drop for Slot {
    fn drop(&mut self) {
        // Listeners detach first (field order is irrelevant: this runs
        // before the fields drop), then nothing more is delivered.
        self.worker.terminate();
    }
}

#[derive(Default)]
struct Pool {
    slots: Vec<Slot>,
    queue: VecDeque<Job>,
    next_id: u32,
    max: usize,
}

thread_local! {
    static POOL: RefCell<Pool> = RefCell::new(Pool::default());
}

fn max_workers() -> usize {
    worker::hardware_concurrency().map_or(4, |n| n as usize).max(1)
}

/// Hand queued jobs to idle workers, starting workers as needed. Never
/// holds the pool borrow across anything that could re-enter it (posting
/// and spawning are synchronous and dispatch no events; a sender's wake
/// schedules a task rather than running it).
fn pump() {
    let mut canceled: Vec<(Job, String)> = Vec::new();
    POOL.with(|p| {
        let mut p = p.borrow_mut();
        if p.max == 0 {
            p.max = max_workers();
        }
        while !p.queue.is_empty() {
            if let Some(i) = p.slots.iter().position(|s| matches!(s.state, State::Idle)) {
                let job = p.queue.pop_front().expect("non-empty");
                let slot = &mut p.slots[i];
                let posted = unsafe {
                    js_post_job(slot.worker.as_js().raw(), job.call, job.job, job.arg.as_ptr() as usize, job.arg.len())
                };
                match posted {
                    Ok(()) => slot.state = State::Busy(Running { name: job.name, tx: job.tx }),
                    Err(e) => canceled.push((job, format!("could not be posted to its worker: {}", e.message()))),
                }
                continue;
            }
            let starting = p.slots.iter().filter(|s| matches!(s.state, State::Starting)).count();
            if starting >= p.queue.len() || p.slots.len() >= p.max {
                break; // a worker on its way, or a full pool: wait for one
            }
            match start_worker(p.next_id) {
                Ok(slot) => {
                    p.next_id = p.next_id.wrapping_add(1);
                    p.slots.push(slot);
                }
                Err(why) => {
                    // Starting another would fail the same way: give the
                    // queued jobs their answer instead of a retry loop.
                    canceled.extend(p.queue.drain(..).map(|j| (j, why.clone())));
                }
            }
        }
    });
    for (job, why) in canceled {
        fail(job.name, &why);
        drop(job.tx);
    }
}

fn start_worker(id: u32) -> Result<Slot, String> {
    let w = worker::spawn(worker_main).map_err(|e| format!("could not start a Web Worker: {}", e.message()))?;
    let on_message = Listener::new(w.clone().into(), "message", ListenerOptions::default(), move |ev: Event| {
        on_reply(id, ev.unchecked_into::<MessageEvent>().data());
    });
    let on_error = Listener::new(w.clone().into(), "error", ListenerOptions::default(), move |ev: Event| {
        // The browser would also report it as uncaught; the log below says
        // which job it was and what happens next instead.
        ev.prevent_default();
        let msg = web_glue::string::receive(|o| unsafe { js_error_message(ev.as_js().raw(), o) });
        on_worker_error(id, &msg);
    });
    Ok(Slot { id, worker: w, state: State::Starting, _listeners: [on_message, on_error] })
}

/// What a worker event leaves to do once the pool borrow is released.
enum After {
    Nothing,
    Deliver(Running, Vec<u8>),
    Fail(Running, String),
    /// A slot taken out of the pool (dropping it terminates the worker),
    /// with what to tell its job, if it had one.
    Retire(Slot, String),
}

fn on_reply(id: u32, data: JsValue) {
    let kind = unsafe { js_reply_kind(data.raw()) };
    let bytes = || -> Vec<u8> {
        let a: web_glue::js::Uint8Array = unsafe { JsValue::from_raw(js_reply_data(data.raw())) }.unchecked_into();
        a.to_vec()
    };
    let payload = if matches!(kind, OK | ERR | PANIC) { bytes() } else { Vec::new() };
    let after = POOL.with(|p| {
        let mut p = p.borrow_mut();
        let Some(i) = p.slots.iter().position(|s| s.id == id) else { return After::Nothing };
        match kind {
            READY => {
                p.slots[i].state = State::Idle;
                After::Nothing
            }
            OK | ERR => match std::mem::replace(&mut p.slots[i].state, State::Idle) {
                State::Busy(r) if kind == OK => After::Deliver(r, payload),
                State::Busy(r) => After::Fail(r, String::from_utf8_lossy(&payload).into_owned()),
                other => {
                    p.slots[i].state = other;
                    After::Nothing
                }
            },
            PANIC => {
                let msg = String::from_utf8_lossy(&payload).into_owned();
                After::Retire(p.slots.remove(i), format!("panicked in its Web Worker: {msg}"))
            }
            _ => After::Nothing,
        }
    });
    finish(after);
    pump();
}

fn finish(after: After) {
    match after {
        After::Nothing => {}
        After::Deliver(r, out) => {
            // Receiver gone (the caller dropped its future): nothing to do.
            let _ = r.tx.send(out);
        }
        After::Fail(r, why) => {
            fail(r.name, &why);
        }
        After::Retire(mut slot, why) => {
            match std::mem::replace(&mut slot.state, State::Idle) {
                State::Busy(r) => {
                    fail(r.name, &why);
                }
                State::Starting => {
                    console::error_1(&JsValue::from_str(&format!("offload: a Web Worker never started ({why})")));
                }
                State::Idle => {
                    console::error_1(&JsValue::from_str(&format!("offload: an idle Web Worker {why}; it is replaced on demand")));
                }
            }
            drop(slot);
        }
    }
}

fn on_worker_error(id: u32, message: &str) {
    let (after, orphaned) = POOL.with(|p| {
        let mut p = p.borrow_mut();
        let Some(i) = p.slots.iter().position(|s| s.id == id) else { return (After::Nothing, Vec::new()) };
        let slot = p.slots.remove(i);
        // The last worker failed before it ever ran a job: another would
        // fail the same way (the module did not load). Answer the queue
        // rather than respawn in a loop.
        let orphaned: Vec<Job> = if matches!(slot.state, State::Starting) && p.slots.is_empty() {
            p.queue.drain(..).collect()
        } else {
            Vec::new()
        };
        (After::Retire(slot, format!("failed in its Web Worker: {message}")), orphaned)
    });
    finish(after);
    for job in orphaned {
        fail(job.name, "was canceled: no Web Worker could be started");
    }
    pump();
}
