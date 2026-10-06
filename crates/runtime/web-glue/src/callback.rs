//! Rust closures callable from JS.
//!
//! A [`Closure`] registers a boxed Rust closure under a fresh `u32` id and
//! mints a JS function (`G.fn(id, flags)`) that calls the one exported
//! entry point, [`__glue_invoke`]`(id, arg_handle)`. Semantics match what
//! the framework relies on from `wasm_bindgen::closure::Closure`:
//!
//! * **Drop revokes.** Dropping the `Closure` unregisters the Rust closure
//!   AND marks the JS function dead. A later JS call throws
//!   `web-glue: callback #N called after its Rust owner dropped it` — a
//!   loud error, never a call into freed memory. Event listeners must be
//!   detached before their `Closure` drops, exactly as today, and frames /
//!   timers cancelled. The throw stays loud on purpose: a callback the host
//!   can still call after its owner dropped it means a missed detach or
//!   cancel, which is a bug to fix at the owner (a fake clock's `1e12` timer
//!   ids once made every cancel miss — see
//!   `dom::Window::request_animation_frame`). The one expected late call is
//!   a promise reaction, since a promise can't be unsubscribed; those are
//!   minted [`FLAG_SILENT`] (a dropped `JsFuture`) and return quietly.
//! * **Ids are never reused** (a monotonically increasing `u32`, panicking
//!   on exhaustion), so even a JS function that escaped revocation could
//!   not reach a different closure that later took its id.
//! * **Re-entrancy is safe.** The closure is taken out of the registry for
//!   the duration of the call, so it may create or drop other callbacks,
//!   or drop ITSELF (the entry is then discarded when the call returns). A
//!   recursive call of the same closure — JS re-entering it from inside
//!   itself — is refused with status 2, which JS turns into a thrown
//!   `invoked recursively` error, as wasm-bindgen does for `FnMut`.
//!   [`Closure::new_fn`] (an `Fn`) is the exception: it may be re-entered,
//!   as wasm-bindgen's `Closure<dyn Fn>` may — a `scroll` handler whose
//!   body synchronously re-fires `scroll` needs exactly that.
//! * [`Closure::once_into_js`] is the fire-and-forget form: no Rust owner,
//!   the entry frees itself after the single call.
//! * [`Closure::into_js_value`] hands the Rust closure to the JS garbage
//!   collector: the runtime registers the function in a
//!   `FinalizationRegistry`, and when JS collects it the runtime calls the
//!   exported [`__glue_release`], which unregisters the closure. That is
//!   `wasm_bindgen::Closure::into_js_value`'s contract, and it is what an
//!   element-lifetime listener needs — the element is the keepalive, and a
//!   discarded element must not pin its listeners' closures for the life of
//!   the page (see `backend_web::primitives::own_listener`).

use std::cell::RefCell;
use std::collections::HashMap;

use crate::{ffi, JsValue};

/// The JS function kills itself after the first call.
pub const FLAG_ONCE: u32 = 1;
/// Calling it after revocation is a silent no-op instead of a throw (promise
/// reactions of a dropped `JsFuture`).
pub const FLAG_SILENT: u32 = 2;

/// JS passes EVERY argument, as one array, instead of only the first
/// ([`Closure::new_with_args`]).
pub const FLAG_ARGS: u32 = 4;

/// `__glue_invoke` statuses. Success is `INVOKE_RETURNED + handle`: the
/// callback's return value rides in the status, so a returning callback
/// costs no second call (`undefined` is handle 0, i.e. status 3).
pub const INVOKE_UNKNOWN: u32 = 1;
pub const INVOKE_RECURSIVE: u32 = 2;
pub const INVOKE_RETURNED: u32 = 3;

enum Kind {
    Mut(Box<dyn FnMut(JsValue) -> JsValue>),
    Once(Box<dyn FnOnce(JsValue) -> JsValue>),
    /// Stays in the registry while it runs (a clone of the `Rc` is what
    /// runs), so a re-entrant call finds it `Idle` and runs too.
    Shared(std::rc::Rc<dyn Fn(JsValue) -> JsValue>),
}

enum Slot {
    Idle(Kind),
    Running,
    /// The owner dropped the closure while it was running; discard it when
    /// the call returns.
    DroppedWhileRunning,
}

struct Registry {
    next: u32,
    map: HashMap<u32, Slot>,
}

thread_local! {
    static REGISTRY: RefCell<Registry> =
        RefCell::new(Registry { next: 1, map: HashMap::new() });
}

fn register(kind: Kind) -> u32 {
    REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let id = r.next;
        r.next = id.checked_add(1).expect("web-glue: callback ids exhausted");
        r.map.insert(id, Slot::Idle(kind));
        id
    })
}

fn unregister(id: u32) {
    let removed = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        match r.map.get_mut(&id) {
            Some(slot @ Slot::Running) => {
                *slot = Slot::DroppedWhileRunning;
                None
            }
            Some(_) => r.map.remove(&id),
            None => None,
        }
    });
    // Dropped outside the borrow: the closure's captures may own other
    // `Closure`s, whose drop re-enters the registry.
    drop(removed);
}

/// A Rust closure JS can call, alive until this value drops.
pub struct Closure {
    id: u32,
    js: JsValue,
}

impl Closure {
    /// A callback JS may call any number of times with one argument
    /// (missing arguments arrive as `undefined`).
    pub fn new(f: impl FnMut(JsValue) + 'static) -> Closure {
        let mut f = f;
        Closure::with_flags(Kind::Mut(Box::new(move |a| {
            f(a);
            JsValue::UNDEFINED
        })), 0)
    }

    /// A callback that receives EVERY argument JS passes (as a slice) and
    /// returns a value to its JS caller — the shape of the virtualizer
    /// shims' row factories and measure callbacks. Refuses re-entry like
    /// [`Closure::new`].
    pub fn new_with_args(f: impl FnMut(&[JsValue]) -> JsValue + 'static) -> Closure {
        let mut f = f;
        Closure::with_flags(
            Kind::Mut(Box::new(move |args: JsValue| {
                let args: crate::js::Array = crate::JsCast::unchecked_into(args);
                f(&args.to_vec())
            })),
            FLAG_ARGS,
        )
    }

    /// A callback JS may call any number of times — INCLUDING from inside
    /// itself (a `scroll` listener whose body re-fires `scroll`). Use it
    /// only where that re-entry is expected; [`Closure::new`] refuses it
    /// loudly, which catches accidental recursion.
    pub fn new_fn(f: impl Fn(JsValue) + 'static) -> Closure {
        Closure::with_flags(Kind::Shared(std::rc::Rc::new(move |a| {
            f(a);
            JsValue::UNDEFINED
        })), 0)
    }

    /// A callback JS may call at most once; the JS function is dead after
    /// the first call.
    pub fn once(f: impl FnOnce(JsValue) + 'static) -> Closure {
        Closure::with_flags(Kind::Once(Box::new(move |a| {
            f(a);
            JsValue::UNDEFINED
        })), FLAG_ONCE)
    }

    /// A one-shot JS function with no Rust owner (`setTimeout`-style
    /// fire-and-forget). Its registry entry frees itself when called; one
    /// that is never called stays registered — the same trade as
    /// `wasm_bindgen::Closure::once_into_js`.
    pub fn once_into_js(f: impl FnOnce(JsValue) + 'static) -> JsValue {
        let id = register(Kind::Once(Box::new(move |a| {
            f(a);
            JsValue::UNDEFINED
        })));
        unsafe { JsValue::from_raw(ffi::make_fn(id, FLAG_ONCE)) }
    }

    /// Give the Rust closure to JS: it lives as long as the returned
    /// function is reachable from JS, and is unregistered (via
    /// [`__glue_release`]) once the function is garbage-collected. The
    /// returned handle may be dropped freely — it is one slab slot, not the
    /// function's lifetime. Calling the function keeps working until then.
    pub fn into_js_value(self) -> JsValue {
        let me = std::mem::ManuallyDrop::new(self);
        // SAFETY: `me` is never dropped, and `js` is read exactly once, so
        // ownership of the slot moves to `js` — the registry entry now
        // belongs to the finalizer instead of to a Rust owner.
        let js = unsafe { std::ptr::read(&me.js) };
        unsafe { ffi::gc_own_fn(js.raw()) };
        js
    }

    pub(crate) fn once_silent(f: impl FnOnce(JsValue) + 'static) -> Closure {
        Closure::with_flags(Kind::Once(Box::new(move |a| {
            f(a);
            JsValue::UNDEFINED
        })), FLAG_ONCE | FLAG_SILENT)
    }

    fn with_flags(kind: Kind, flags: u32) -> Closure {
        let id = register(kind);
        let js = unsafe { JsValue::from_raw(ffi::make_fn(id, flags)) };
        Closure { id, js }
    }

    /// The JS function — pass it to `addEventListener`, `then`, ….
    pub fn as_js(&self) -> &JsValue {
        &self.js
    }

    /// The registry id (diagnostics).
    pub fn id(&self) -> u32 {
        self.id
    }

    /// Rust closures currently registered on this thread.
    pub fn live_count() -> usize {
        REGISTRY.with(|r| r.borrow().map.len())
    }
}

impl Drop for Closure {
    fn drop(&mut self) {
        unregister(self.id);
        let js = std::mem::replace(&mut self.js, JsValue::undefined());
        unsafe { ffi::revoke_fn(js.into_raw()) }
    }
}

impl std::fmt::Debug for Closure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Closure(#{})", self.id)
    }
}

/// The runtime's `FinalizationRegistry` callback for a function minted by
/// [`Closure::into_js_value`]: JS collected it, so nothing can call it any
/// more — drop the Rust closure. A no-op for an id already gone (a once
/// callback that ran).
#[unsafe(no_mangle)]
pub extern "C" fn __glue_release(id: u32) {
    unregister(id);
}

/// The one entry point JS calls callbacks through. Takes ownership of the
/// `arg` slot. Returns [`INVOKE_RETURNED`] `+` the handle of the callback's
/// return value (JS takes it), [`INVOKE_UNKNOWN`] (not registered: dropped,
/// or a once-callback already spent) or [`INVOKE_RECURSIVE`].
#[unsafe(no_mangle)]
pub extern "C" fn __glue_invoke(id: u32, arg: u32) -> u32 {
    // Owned from here on, so every early return releases it.
    let arg = unsafe { JsValue::from_raw(arg) };
    let taken = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let Some(slot) = r.map.get_mut(&id) else { return Err(INVOKE_UNKNOWN) };
        if let Slot::Idle(Kind::Shared(f)) = slot {
            return Ok(Kind::Shared(f.clone()));
        }
        match std::mem::replace(slot, Slot::Running) {
            Slot::Idle(Kind::Once(f)) => {
                r.map.remove(&id);
                Ok(Kind::Once(f))
            }
            Slot::Idle(kind) => Ok(kind),
            other => {
                *slot = other;
                Err(INVOKE_RECURSIVE)
            }
        }
    });
    match taken {
        Err(status) => status,
        Ok(Kind::Once(f)) => INVOKE_RETURNED + f(arg).into_raw(),
        // The registry keeps its own `Rc`; this clone is dropped here, so
        // an owner that dropped the closure mid-call frees it now.
        Ok(Kind::Shared(f)) => INVOKE_RETURNED + f(arg).into_raw(),
        Ok(Kind::Mut(mut f)) => {
            let ret = f(arg);
            let discard = REGISTRY.with(|r| {
                let mut r = r.borrow_mut();
                match r.map.get_mut(&id) {
                    Some(slot @ Slot::Running) => {
                        *slot = Slot::Idle(Kind::Mut(f));
                        None
                    }
                    Some(Slot::DroppedWhileRunning) => {
                        r.map.remove(&id);
                        Some(f)
                    }
                    // Unreachable by construction; keep the closure alive
                    // no longer than this call rather than panic in a
                    // callback.
                    _ => Some(f),
                }
            });
            drop(discard);
            INVOKE_RETURNED + ret.into_raw()
        }
    }
}
