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
//!   detached before their `Closure` drops, exactly as today.
//! * **Ids are never reused** (a monotonically increasing `u32`, panicking
//!   on exhaustion), so even a JS function that escaped revocation could
//!   not reach a different closure that later took its id.
//! * **Re-entrancy is safe.** The closure is taken out of the registry for
//!   the duration of the call, so it may create or drop other callbacks,
//!   or drop ITSELF (the entry is then discarded when the call returns). A
//!   recursive call of the same closure — JS re-entering it from inside
//!   itself — is refused with status 2, which JS turns into a thrown
//!   `invoked recursively` error, as wasm-bindgen does.
//! * [`Closure::once_into_js`] is the fire-and-forget form: no Rust owner,
//!   the entry frees itself after the single call.

use std::cell::RefCell;
use std::collections::HashMap;

use crate::{ffi, JsValue};

/// The JS function kills itself after the first call.
pub const FLAG_ONCE: u32 = 1;
/// Calling it after revocation is a silent no-op instead of a throw (promise
/// reactions of a dropped `JsFuture`).
pub const FLAG_SILENT: u32 = 2;

/// `__glue_invoke` statuses.
pub const INVOKE_OK: u32 = 0;
pub const INVOKE_UNKNOWN: u32 = 1;
pub const INVOKE_RECURSIVE: u32 = 2;

enum Kind {
    Mut(Box<dyn FnMut(JsValue)>),
    Once(Box<dyn FnOnce(JsValue)>),
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
        Closure::with_flags(Kind::Mut(Box::new(f)), 0)
    }

    /// A callback JS may call at most once; the JS function is dead after
    /// the first call.
    pub fn once(f: impl FnOnce(JsValue) + 'static) -> Closure {
        Closure::with_flags(Kind::Once(Box::new(f)), FLAG_ONCE)
    }

    /// A one-shot JS function with no Rust owner (`setTimeout`-style
    /// fire-and-forget). Its registry entry frees itself when called; one
    /// that is never called stays registered — the same trade as
    /// `wasm_bindgen::Closure::once_into_js`.
    pub fn once_into_js(f: impl FnOnce(JsValue) + 'static) -> JsValue {
        let id = register(Kind::Once(Box::new(f)));
        unsafe { JsValue::from_raw(ffi::make_fn(id, FLAG_ONCE)) }
    }

    pub(crate) fn once_silent(f: impl FnOnce(JsValue) + 'static) -> Closure {
        Closure::with_flags(Kind::Once(Box::new(f)), FLAG_ONCE | FLAG_SILENT)
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

/// The one entry point JS calls callbacks through. Takes ownership of the
/// `arg` slot. Returns [`INVOKE_OK`], [`INVOKE_UNKNOWN`] (not registered:
/// dropped, or a once-callback already spent) or [`INVOKE_RECURSIVE`].
#[unsafe(no_mangle)]
pub extern "C" fn __glue_invoke(id: u32, arg: u32) -> u32 {
    // Owned from here on, so every early return releases it.
    let arg = unsafe { JsValue::from_raw(arg) };
    let taken = REGISTRY.with(|r| {
        let mut r = r.borrow_mut();
        let Some(slot) = r.map.get_mut(&id) else { return Err(INVOKE_UNKNOWN) };
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
        Ok(Kind::Once(f)) => {
            f(arg);
            INVOKE_OK
        }
        Ok(Kind::Mut(mut f)) => {
            f(arg);
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
            INVOKE_OK
        }
    }
}
