//! Web Workers that run THIS module: [`spawn`] starts a module Worker that
//! instantiates the same wasm (sharing no memory) and calls a Rust `fn()`
//! in it.
//!
//! # How a worker finds the module
//!
//! The generated JS that embeds the runtime (own mode: `pkg/<lib>.js`;
//! hybrid mode: `pkg/__idealyst_glue.js`) reports its own `import.meta.url`
//! to the runtime (`G.entry`) and exports `__glueWorkerInit(module?)`, which
//! instantiates the module and resolves to its raw exports. The worker is a
//! small bootstrap from a blob URL: it `import()`s that URL, calls
//! `__glueWorkerInit`, then the export [`__glue_worker_start`] with the
//! entry's function-table index. Instantiating runs the module's start
//! (`main` for a bin); idealyst's web boot returns early where there is no
//! `window`, so an app's `main` mounts nothing in a worker.
//!
//! # Why a function pointer is enough
//!
//! A Rust `fn` pointer on wasm32 IS an index into the module's function
//! table, and the table's initial contents come from the module's element
//! segments — so two instances of the same module bytes agree on every
//! index. The worker's `call_indirect` is also type-checked by the engine:
//! a stale or foreign index traps (an `error` event on the [`Worker`]); it
//! can never run arbitrary code with the wrong signature.
//!
//! Two cases where the main thread's index has no twin in the worker, both
//! of which trap rather than misbehave: a function only reachable through a
//! wasm-split chunk the worker never loaded, and — in a dev session — a
//! function a hot patch added (a patched body of an EXISTING function keeps
//! its index; the worker runs the base build's body until a reload).
//!
//! # Readiness
//!
//! The worker is not listening for messages until the entry fn installs a
//! listener (a message posted earlier is dropped by the browser), so an
//! entry that wants messages should post one first ("ready") and the
//! parent should wait for it before posting work.

use crate::cast::JsCast;
use crate::dom::EventTarget;
use crate::{JsError, JsValue};

crate::js_class! {
    /// `Worker` — a dedicated worker, seen from the thread that started it.
    pub struct Worker: EventTarget = "Worker";
}

crate::import! {
    #[catch]
    fn js_spawn_worker(entry: u32) -> u32 = "(e) => G.spawnWorker(e)";
    #[catch]
    fn js_worker_post(w: u32, msg: u32, transfer: u32) =
        "(w, m, t) => { G.get(w).postMessage(G.get(m), G.get(t) ?? []); }";
    fn js_worker_terminate(w: u32) = "(w) => { G.get(w).terminate(); }";
    fn js_in_worker() -> u32 =
        "() => (typeof WorkerGlobalScope !== 'undefined' && globalThis instanceof WorkerGlobalScope) ? 1 : 0";
    #[catch]
    fn js_post_to_parent(msg: u32, transfer: u32) =
        "(m, t) => { globalThis.postMessage(G.get(m), G.get(t) ?? []); }";
    fn js_hardware_concurrency() -> u32 =
        "() => { const n = globalThis.navigator && globalThis.navigator.hardwareConcurrency; \
           return (typeof n === 'number' && n > 0) ? n >>> 0 : 0; }";
}

/// Start a Worker running `entry` in a fresh instance of this module.
///
/// Fails when the bundle's JS predates worker support (it does not report
/// its URL) or the browser refuses the Worker. A failure INSIDE the worker
/// — the module failing to load, `entry` trapping or panicking — arrives as
/// the returned Worker's `error` event.
pub fn spawn(entry: fn()) -> Result<Worker, JsError> {
    let index = entry as usize as u32;
    let raw = unsafe { js_spawn_worker(index) }?;
    Ok(Worker::unchecked_from_js(unsafe { JsValue::from_raw(raw) }))
}

impl Worker {
    /// `postMessage(msg)`.
    pub fn post_message(&self, msg: &JsValue) -> Result<(), JsError> {
        unsafe { js_worker_post(self.as_js().raw(), msg.raw(), 0) }
    }

    /// `postMessage(msg, transfer)` — `transfer` is an array of
    /// transferables (e.g. an `ArrayBuffer`), detached on this side.
    pub fn post_message_with_transfer(&self, msg: &JsValue, transfer: &JsValue) -> Result<(), JsError> {
        unsafe { js_worker_post(self.as_js().raw(), msg.raw(), transfer.raw()) }
    }

    /// `terminate()` — stops the worker at once; nothing more is delivered
    /// from it.
    pub fn terminate(&self) {
        unsafe { js_worker_terminate(self.as_js().raw()) }
    }
}

/// Whether this code runs in a worker (`globalThis` is a
/// `WorkerGlobalScope`).
pub fn in_worker() -> bool {
    unsafe { js_in_worker() != 0 }
}

/// `globalThis` as an [`EventTarget`] — in a worker, where `message` events
/// from the parent arrive.
pub fn scope() -> EventTarget {
    EventTarget::unchecked_from_js(JsValue::global())
}

/// From inside a worker: `postMessage(msg)` to the thread that started it.
pub fn post_to_parent(msg: &JsValue) -> Result<(), JsError> {
    unsafe { js_post_to_parent(msg.raw(), 0) }
}

/// From inside a worker: `postMessage(msg, transfer)` to the parent.
pub fn post_to_parent_with_transfer(msg: &JsValue, transfer: &JsValue) -> Result<(), JsError> {
    unsafe { js_post_to_parent(msg.raw(), transfer.raw()) }
}

/// `navigator.hardwareConcurrency`, or `None` where the browser does not
/// report it.
pub fn hardware_concurrency() -> Option<u32> {
    match unsafe { js_hardware_concurrency() } {
        0 => None,
        n => Some(n),
    }
}

/// The worker bootstrap's entry: run the `fn()` at function-table index
/// `entry`. Called once per worker, by the bootstrap in `js/runtime.js`
/// (`WORKER_BOOT`), right after the module is instantiated.
#[unsafe(no_mangle)]
pub extern "C" fn __glue_worker_start(entry: u32) {
    assert!(entry != 0, "web-glue: worker started with a null entry");
    // SAFETY: `entry` is the table index of a `fn()` in the parent's
    // instance of this same module (`spawn` produced it from a `fn()`), so
    // it names the same function here — see the module docs. A foreign
    // index cannot cause UB: `call_indirect` checks the signature and traps.
    let f: fn() = unsafe { core::mem::transmute::<usize, fn()>(entry as usize) };
    f();
}
