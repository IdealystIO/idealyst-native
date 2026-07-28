//! Live signal-watch registry for the NEW core — the vocabulary port of
//! `runtime_core::robot::watch` (P5 `watch_signal` seam). Whole module
//! compiles only under the vocabulary `robot` feature.
//!
//! # Author surface (unchanged from the old core)
//!
//! An author explicitly marks a signal with
//! [`watch_signal`]`("name", sig)` (value type must be `Debug`); the
//! bridge verbs `list_watched_signals` / `read_signal` then expose the
//! live value, and `robot-test`'s `app.signal("name").assert_eq(…)`
//! rides those verbs unchanged. Auto-watching every signal is as
//! impossible here as it was on the old core (rendering needs `Debug`,
//! which not every signal type has); the old `signal!` macro is gone,
//! so the explicit fn IS the whole surface.
//!
//! # Staleness model: scope-tied entries, not generation checks
//!
//! The old registry guarded recycled arena slots with a generation
//! check (`signal_is_live`) because its reads tolerated dead handles.
//! `runtime_world` reads PANIC on a stale handle and exposes no public
//! liveness probe — so entries must never outlive their signal instead.
//! [`watch_signal`] arms an [`on_teardown`](crate::style_attach::on_teardown)
//! probe in the ambient collector: called from a component body (the
//! normal place), the entry dies with the component's `Owned` — i.e.
//! strictly before any later read could touch the freed slot. Called
//! from app-root build code, the probe is world-root-owned and the
//! entry dies at world drop (`newcore::stop`), same as the signal.
//! Consequence, documented on purpose: `watch_signal` must run where
//! effect creation is legal (a component body, an effect, or any
//! world-entered build scope) — the same contract every other new-core
//! registration has.
//!
//! The teardown probe removes the entry only if it still carries the
//! registering handle's FULL `raw_id` (generation included): a later
//! registration that reused the slot id (last-wins, mirroring the
//! element registry's `by_test_id` policy) must not be orphaned by the
//! older entry's teardown.
//!
//! # Wire ids
//!
//! Entries key by the signal's 32-bit slot id on the wire (`raw_id`'s
//! low half) — the old registry's exact id shape, and safe through
//! JS-side JSON relays (a full `raw_id` packs the world id above bit
//! 53, where JSON number precision dies).

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;

use runtime_core::__serde_json as serde_json;
use runtime_world::{untrack, Memo, ReadSignal, Signal};
use serde_json::Value;

/// Anything watchable: the unified handle, the read half, or a memo
/// output. Sealed-ish by construction (implemented for exactly the
/// kernel's read-capable handles).
pub trait WatchTarget<T> {
    /// Full identity key (world | generation | slot) — see
    /// `runtime_world::Signal::raw_id`.
    fn watch_raw_id(&self) -> u64;
    /// Untracked `Debug` render of the current value.
    fn watch_read(&self) -> String;
}

impl<T: PartialEq + Clone + Debug + 'static> WatchTarget<T> for Signal<T> {
    fn watch_raw_id(&self) -> u64 {
        self.raw_id()
    }
    fn watch_read(&self) -> String {
        untrack(|| self.with(|v| format!("{v:?}")))
    }
}

impl<T: PartialEq + Clone + Debug + 'static> WatchTarget<T> for ReadSignal<T> {
    fn watch_raw_id(&self) -> u64 {
        self.raw_id()
    }
    fn watch_read(&self) -> String {
        untrack(|| self.with(|v| format!("{v:?}")))
    }
}

impl<T: PartialEq + Clone + Debug + 'static> WatchTarget<T> for Memo<T> {
    fn watch_raw_id(&self) -> u64 {
        self.raw_id()
    }
    fn watch_read(&self) -> String {
        untrack(|| format!("{:?}", self.get()))
    }
}

struct WatchEntry {
    name: String,
    /// Full identity of the registering handle — teardown removes the
    /// entry only while it still owns the slot (module docs).
    raw_id: u64,
    /// Untracked read → JSON. Only invoked while the entry is alive,
    /// which the scope-tied teardown guarantees means the slot is live.
    reader: Rc<dyn Fn() -> Value>,
}

thread_local! {
    /// Slot id (u32, the wire id) → entry. At most one watch per slot;
    /// re-registration on the same slot overwrites (last-wins).
    static WATCHED: RefCell<HashMap<u32, WatchEntry>> = RefCell::new(HashMap::new());
}

/// Register a signal (or read half / memo) for live watching over the
/// robot bridge. `T: Debug` renders the value; reads are untracked so a
/// robot query never subscribes anything. Calling twice on the same
/// slot replaces the prior entry.
///
/// Must run where effect creation is legal (component body / effect /
/// world-entered build) — the entry's lifetime is tied to the ambient
/// scope (module docs).
pub fn watch_signal<T, S>(name: impl Into<String>, signal: S)
where
    T: PartialEq + Clone + Debug + 'static,
    S: WatchTarget<T> + 'static,
{
    let raw_id = signal.watch_raw_id();
    let slot = (raw_id & 0xffff_ffff) as u32;
    let reader: Rc<dyn Fn() -> Value> =
        Rc::new(move || Value::String(signal.watch_read()));
    WATCHED.with(|w| {
        w.borrow_mut().insert(
            slot,
            WatchEntry {
                name: name.into(),
                raw_id,
                reader,
            },
        );
    });
    // Scope-tied removal (module docs). Guarded on the full raw_id so a
    // newer same-slot registration survives this entry's teardown.
    crate::style_attach::on_teardown(move || {
        WATCHED.with(|w| {
            let mut w = w.borrow_mut();
            if w.get(&slot).is_some_and(|e| e.raw_id == raw_id) {
                w.remove(&slot);
            }
        });
    });
}

/// One watched signal's current state (`list_watched_signals` verb).
pub struct WatchedSnapshot {
    pub id: u32,
    pub name: String,
    pub value: Value,
}

/// Snapshot every watched signal with its current value, name-sorted
/// for a stable inspector display. Readers run after the registry
/// borrow drops (a reader could defensively re-enter this module) and
/// world-ENTERED via the installed driver env (self-wrapping, like the
/// Robot's label queries — the harness/bridge need not wrap).
pub fn list_watched() -> Vec<WatchedSnapshot> {
    let entries: Vec<(u32, String, Rc<dyn Fn() -> Value>)> = WATCHED.with(|w| {
        w.borrow()
            .iter()
            .map(|(id, e)| (*id, e.name.clone(), e.reader.clone()))
            .collect()
    });
    let mut out: Vec<WatchedSnapshot> = crate::robot::entered(|| {
        entries
            .into_iter()
            .map(|(id, name, reader)| WatchedSnapshot {
                id,
                name,
                value: reader(),
            })
            .collect()
    });
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    out
}

/// Read one watched signal's current value by wire slot id. Runs
/// entered (driver env), like every robot query.
pub fn read_watched_by_id(id: u32) -> Option<Value> {
    let reader = WATCHED.with(|w| w.borrow().get(&id).map(|e| e.reader.clone()))?;
    Some(crate::robot::entered(|| reader()))
}

/// Read one watched signal's current value by name. Runs entered
/// (driver env), like every robot query.
pub fn read_watched_by_name(name: &str) -> Option<Value> {
    let reader = WATCHED.with(|w| {
        w.borrow()
            .iter()
            .find(|(_, e)| e.name == name)
            .map(|(_, e)| e.reader.clone())
    })?;
    Some(crate::robot::entered(|| reader()))
}

/// Stop watching a signal by wire slot id. No-op if absent.
pub fn unwatch_signal(id: u32) {
    WATCHED.with(|w| {
        w.borrow_mut().remove(&id);
    });
}

/// Test isolation: empty the registry.
pub(crate) fn reset() {
    WATCHED.with(|w| w.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;
    use runtime_world::{signal, World};

    /// watch → write → read-by-name/id returns the live Debug value;
    /// unwatch removes. The read runs world-entered like the bridge
    /// verbs do.
    #[test]
    fn watch_then_read_returns_live_debug_value() {
        reset();
        let world = World::new();
        world.enter(|| {
            let s = signal(1i32);
            watch_signal("counter", s);
            s.set(42);
        });
        world.flush();
        world.enter(|| {
            assert_eq!(
                read_watched_by_name("counter"),
                Some(serde_json::json!("42"))
            );
            let list = list_watched();
            let row = list.iter().find(|w| w.name == "counter").expect("listed");
            assert_eq!(row.value, serde_json::json!("42"));
            assert_eq!(read_watched_by_id(row.id), Some(serde_json::json!("42")));
            unwatch_signal(row.id);
            assert!(read_watched_by_name("counter").is_none());
        });
        reset();
    }

    /// The staleness model: an entry registered inside a component
    /// scope dies with that scope's `Owned` — a read after teardown
    /// finds no entry (it must NOT reach the freed slot, which would
    /// panic in the kernel).
    #[test]
    fn regression_scope_teardown_removes_watch_entry_before_slot_frees() {
        reset();
        let world = World::new();
        world.enter(|| {
            let (_, owned) = runtime_world::collect_owned(|| {
                let s = signal(7i32);
                watch_signal("scoped", s);
            });
            assert!(read_watched_by_name("scoped").is_some(), "live while owned");
            drop(owned); // frees the effect (probe fires) AND the signal
            assert!(
                read_watched_by_name("scoped").is_none(),
                "entry must die with the scope — a stale read would panic on the freed slot"
            );
        });
        reset();
    }

    /// Last-wins on a reused slot id: the OLDER entry's teardown must
    /// not orphan a newer registration that took over the slot.
    #[test]
    fn stale_teardown_does_not_orphan_newer_same_slot_entry() {
        reset();
        let world = World::new();
        world.enter(|| {
            let (first_slot, owned) = runtime_world::collect_owned(|| {
                let s = signal(1i32);
                watch_signal("first", s);
                (s.raw_id() & 0xffff_ffff) as u32
            });
            drop(owned); // frees the slot; probe removes "first"
            // New signal reuses the freed slot (kernel freelist).
            let s2 = signal(2i32);
            let second_slot = (s2.raw_id() & 0xffff_ffff) as u32;
            assert_eq!(first_slot, second_slot, "slot must recycle for this test");
            watch_signal("second", s2);
            assert_eq!(
                read_watched_by_name("second"),
                Some(serde_json::json!("2")),
                "newer same-slot entry stays live"
            );
        });
        reset();
    }
}
