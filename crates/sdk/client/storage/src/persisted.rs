//! `persisted_signal` — the blessed storage↔signal rail.
//!
//! Why this exists (arena findings, 2026-07-21): every agent hand-rolls
//! load/persist wiring, and the natural shape is racy — an unconditional
//! `sig.set(loaded)` after an async load CLOBBERS anything the user did
//! before the load resolved, and a "loaded" gate on the persist side
//! silently drops those same writes. Two independent implementations
//! (todo-app runs 2 and 3) both shipped variants of that race; the
//! adversary review flagged it as a determinism defect. This helper makes
//! the correct behavior the only behavior:
//!
//! * **User writes win.** Hydration applies ONLY if the signal hasn't been
//!   written since creation. A pre-hydration write both sticks visually and
//!   persists — deterministic last-write-wins from the user's perspective;
//!   the stale stored value is superseded, never resurrected.
//! * **Persist-on-change is automatic** (JSON via serde), with the initial
//!   value never echoed to storage on creation (a fresh app that writes
//!   nothing stores nothing).
//! * **Corrupt stored payloads are ignored**, retaining the initial value —
//!   and NOT overwritten until the user actually changes something.
//!
//! Lifetime: the persist watcher is leaked to match the signal's own
//! lifetime (a persisted signal is app-level state; once the owning world
//! drops, writes through the leaked watcher are inert no-ops). Create it in a
//! component body — signal AND effect creation both require the ambient
//! world, so this is not callable from an event handler.
//!
//! Timing: writes STAGE. `sig.set(v)` is committed by the driver's flush, and
//! the persist watcher runs during that flush — so the storage write is
//! issued after the turn that wrote the signal, not inside it.

use crate::{platform_storage, Storage};
use runtime_core::driver::spawn_async;
use runtime_core::{signal, watch, Signal};
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) struct PersistState {
    /// A user write happened since creation → hydration must yield.
    touched: Cell<bool>,
    /// The write currently flowing through the watcher is the hydration
    /// apply itself, not a user write.
    applying: Cell<bool>,
    /// `watch` runs its body once at creation; that run must neither
    /// persist the initial value nor count as a user write.
    first: Cell<bool>,
}

impl PersistState {
    fn new() -> Rc<Self> {
        Rc::new(Self {
            touched: Cell::new(false),
            applying: Cell::new(false),
            first: Cell::new(true),
        })
    }
}

/// Apply a loaded value to the signal iff the user hasn't written since
/// creation. Extracted so the race contract is unit-testable without an
/// async gate: "touched → hydration yields" is the whole determinism story.
pub(crate) fn apply_hydration<T: Clone + PartialEq + 'static>(
    sig: Signal<T>,
    st: &PersistState,
    loaded: T,
) {
    if st.touched.get() {
        return; // user got there first — their state wins, stays persisted
    }
    st.applying.set(true);
    sig.set(loaded);
    st.applying.set(false);
}

/// A signal whose value persists in [`platform_storage`] under
/// `(namespace, key)` and hydrates from it on creation. See the module docs
/// for the exact race contract. Requires the `reactive` feature.
pub fn persisted_signal<T>(namespace: &str, key: &str, initial: T) -> Signal<T>
where
    T: Clone + PartialEq + Serialize + DeserializeOwned + 'static,
{
    persisted_signal_with(platform_storage(namespace), key, initial)
}

/// [`persisted_signal`] with an explicit store — the seam tests (and
/// callers with a custom [`Storage`]) use.
pub fn persisted_signal_with<T>(store: Arc<dyn Storage>, key: &str, initial: T) -> Signal<T>
where
    T: Clone + PartialEq + Serialize + DeserializeOwned + 'static,
{
    let sig = signal(initial);
    let st = PersistState::new();
    let key_owned = key.to_string();

    // Persist-on-change. NOTE: if the reactive flush defers this watcher
    // past `apply_hydration`'s applying-window, the hydrated value is
    // persisted back once — a harmless echo of data already in storage.
    {
        let st = st.clone();
        let store = store.clone();
        let key = key_owned.clone();
        watch(move || {
            let val = sig.get(); // tracked read — refire on every write
            if st.first.replace(false) {
                return; // creation run: initial value is not user data
            }
            if st.applying.get() {
                return; // hydration apply is not a user write
            }
            st.touched.set(true);
            let Ok(raw) = serde_json::to_string(&val) else {
                return;
            };
            let store = store.clone();
            let key = key.clone();
            spawn_async(async move {
                let _ = store.set(&key, &raw).await;
            });
        })
        .leak();
    }

    // Hydrate. On web this races user input (that's the point of the
    // touched guard); on native the executor may run it inline at creation.
    {
        let st = st.clone();
        spawn_async(async move {
            if let Ok(Some(raw)) = store.get(&key_owned).await {
                if let Ok(v) = serde_json::from_str::<T>(&raw) {
                    apply_hydration(sig, &st, v);
                }
                // Unparseable payload: keep the initial value and DON'T
                // touch storage — the corrupt blob survives for forensics
                // until the user actually writes something new.
            }
        });
    }

    sig
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MemoryStorage;

    fn mem_with(key: &str, raw: &str) -> Arc<dyn Storage> {
        let m = MemoryStorage::new();
        pollster::block_on(m.set(key, raw)).unwrap();
        Arc::new(m)
    }

    /// Signals + effects are per-world and can only be created inside
    /// `World::enter`; writes stage until a flush. `world(|| …)` supplies the
    /// world and commits at the end, `flush()` commits mid-body (what the
    /// backend's driver does after every handler).
    fn world<R>(f: impl FnOnce() -> R) -> R {
        runtime_core::__with_fresh_world(f)
    }

    fn flush() {
        runtime_core::__flush_test_world();
    }

    #[test]
    fn hydrates_saved_value_when_untouched() {
        world(|| {
            let store = mem_with("n", "\"saved\"");
            let sig = persisted_signal_with::<String>(store, "n", "initial".into());
            // Native spawn_async is inline → hydration already staged.
            flush();
            assert_eq!(sig.peek(), "saved");
        });
    }

    #[test]
    fn regression_hydration_yields_to_user_write() {
        // The arena-found race: user writes before the async load resolves;
        // hydration must NOT clobber. Exercised via the extracted apply step
        // (the async path can't be held open under the inline executor).
        world(|| {
            let sig = signal(String::from("user-typed"));
            let st = PersistState::new();
            st.touched.set(true); // a user write happened pre-hydration
            apply_hydration(sig, &st, String::from("stale-saved"));
            flush();
            assert_eq!(
                sig.peek(),
                "user-typed",
                "hydration must yield to the user's pre-load write"
            );
        });
    }

    #[test]
    fn user_change_persists_and_initial_does_not() {
        world(|| {
            let mem = Arc::new(MemoryStorage::new());
            let store: Arc<dyn Storage> = mem.clone();
            let sig = persisted_signal_with::<String>(store.clone(), "k", "initial".into());
            flush();
            // Creation alone stores nothing.
            assert_eq!(pollster::block_on(store.get("k")).unwrap(), None);
            // A user write persists once the turn commits and the watcher
            // runs (inline executor → the storage write lands in the flush).
            sig.set("typed".into());
            flush();
            assert_eq!(
                pollster::block_on(store.get("k")).unwrap().as_deref(),
                Some("\"typed\"")
            );
        });
    }

    #[test]
    fn corrupt_payload_keeps_initial_and_storage_untouched() {
        world(|| {
            let store = mem_with("k", "not-json{{{");
            let sig = persisted_signal_with::<String>(store.clone(), "k", "initial".into());
            flush();
            assert_eq!(sig.peek(), "initial");
            assert_eq!(
                pollster::block_on(store.get("k")).unwrap().as_deref(),
                Some("not-json{{{"),
                "corrupt blob preserved until a real user write"
            );
        });
    }
}
