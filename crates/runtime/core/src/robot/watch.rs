//! Live signal-watch registry — the introspection layer that lets the
//! inspector read signal values over the robot bridge.
//!
//! # Why a registry
//!
//! Signal values are stored type-erased (`Box<dyn Any>`, see
//! `reactive::Arena`), so an external controller that speaks JSON can't
//! turn a bare `SignalId` back into a value — only code holding the
//! typed `Signal<T>` handle can. This registry captures, at the point
//! where `T` is concrete, a small reader closure `Fn() -> Value` that
//! renders the value (via `Debug`) alongside a human name.
//!
//! Watching is **explicit**: an author marks a signal with
//! [`watch_signal`] (the value type must be `Debug`). Auto-watching every
//! `signal!` was considered but is impossible to do safely in stable
//! Rust — rendering a value needs `Debug`, forcing that on every signal
//! breaks the many signals over non-`Debug` types, and the "use Debug if
//! present" specialization trick isn't inference-safe (it fails to
//! compile for inference-deferred types like `signal!(None)`).
//!
//! # Generational safety
//!
//! Arena slots are recycled, and their generation is bumped on free
//! (see `Arena::signal_gen` / [[project_generational_signal_handles]]).
//! Each entry records the generation captured at registration time;
//! every read re-checks it against the slot's *current* generation
//! ([`crate::reactive::signal_is_live`]) and skips + prunes stale
//! entries. A recycled slot therefore never serves a different signal's
//! value through a leftover watch entry. Entries are also dropped
//! eagerly when the slot is freed, via [`on_signal_freed`] (called from
//! the arena's batched-free path, beside the JS-notifier cleanup).
//!
//! Entire module compiles only under the `robot` feature.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Debug;
use std::rc::Rc;

use serde_json::Value;

use crate::reactive::Signal;

struct WatchEntry {
    name: String,
    /// Generation the watched handle was minted with — guards against a
    /// recycled slot (see module docs).
    gen: u32,
    /// Reads the live value and renders it as JSON. Only invoked after a
    /// liveness+generation check, so it never trips `Signal::get`'s
    /// stale-read panic.
    reader: Rc<dyn Fn() -> Value>,
}

thread_local! {
    /// Raw `SignalId` (slot index, `u32`) → entry. At most one watch per
    /// slot; re-registration on the same slot overwrites (the macro path
    /// can re-mark a recycled slot with a fresh generation).
    static WATCHED: RefCell<HashMap<u32, WatchEntry>> = RefCell::new(HashMap::new());
}

/// Register a signal for live watching over the robot bridge (explicit
/// escape hatch — reaches framework-internal signals the `signal!` macro
/// never sees). Requires `T: Debug`: an author explicitly naming a
/// signal to watch knows its value type and almost always wants its
/// `Debug` form, and a `fn` (unlike the macro) can't do the
/// non-`Debug`/`Debug` specialization. Non-`Debug` signals are still
/// auto-registered (as `<opaque>`) by the `signal!` macro.
///
/// Calling twice on the same slot replaces the prior entry. No-op in
/// non-`robot` builds (the whole module is gated).
pub fn watch_signal<T: Clone + Debug + 'static>(name: impl Into<String>, signal: Signal<T>) {
    let reader: Rc<dyn Fn() -> Value> = Rc::new(move || {
        // Untracked: querying must never subscribe a scope. Liveness is
        // checked by the read path before this runs, so `get()` won't
        // hit its stale-read panic.
        let value = crate::reactive::untrack(|| signal.get());
        Value::String(format!("{:?}", value))
    });
    record_watch_reader(
        name.into(),
        signal.id() as u32,
        crate::reactive::signal_generation(signal.id()),
        reader,
    );
}

/// Store a pre-built reader for a signal slot. [`watch_signal`] builds
/// the reader at a concrete-`T` site, then type-erases it to
/// `Fn() -> Value`. `gen` is the slot's generation at registration; the
/// read paths re-check it so a recycled slot never serves a stale value.
pub(crate) fn record_watch_reader(
    name: String,
    id: u32,
    gen: u32,
    reader: Rc<dyn Fn() -> Value>,
) {
    WATCHED.with(|w| {
        w.borrow_mut().insert(id, WatchEntry { name, gen, reader });
    });
}

/// One watched signal's current state, for the `list_watched_signals`
/// bridge verb.
pub struct WatchedSnapshot {
    pub id: u32,
    pub name: String,
    pub value: Value,
}

/// Snapshot every currently-live watched signal with its current value.
/// Stale entries (slot freed/recycled — generation mismatch) are skipped
/// and pruned. Results are name-sorted for a stable inspector display.
pub fn list_watched() -> Vec<WatchedSnapshot> {
    // Clone the (id, name, gen, reader) tuples out under a short borrow,
    // then drop the borrow before invoking readers — a reader touches the
    // arena and (defensively) could re-enter this module, so we never
    // hold the `WATCHED` borrow across a reader call.
    let entries: Vec<(u32, String, u32, Rc<dyn Fn() -> Value>)> = WATCHED.with(|w| {
        w.borrow()
            .iter()
            .map(|(id, e)| (*id, e.name.clone(), e.gen, e.reader.clone()))
            .collect()
    });

    let mut out = Vec::with_capacity(entries.len());
    let mut stale = Vec::new();
    for (id, name, gen, reader) in entries {
        if crate::reactive::signal_is_live(id as u64, gen) {
            out.push(WatchedSnapshot { id, name, value: reader() });
        } else {
            stale.push(id);
        }
    }
    if !stale.is_empty() {
        WATCHED.with(|w| {
            let mut w = w.borrow_mut();
            for id in stale {
                w.remove(&id);
            }
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    out
}

/// Read one watched signal's current value by raw slot id. `None` if
/// absent or stale (recycled slot). Prunes a stale entry on the way out.
pub fn read_watched_by_id(id: u32) -> Option<Value> {
    let entry = WATCHED.with(|w| {
        w.borrow().get(&id).map(|e| (e.gen, e.reader.clone()))
    })?;
    if crate::reactive::signal_is_live(id as u64, entry.0) {
        Some(entry.1())
    } else {
        WATCHED.with(|w| {
            w.borrow_mut().remove(&id);
        });
        None
    }
}

/// Read one watched signal's current value by name. `None` if no live
/// entry has that name.
pub fn read_watched_by_name(name: &str) -> Option<Value> {
    let id = WATCHED.with(|w| {
        w.borrow()
            .iter()
            .find(|(_, e)| e.name == name)
            .map(|(id, _)| *id)
    })?;
    read_watched_by_id(id)
}

/// Stop watching a signal by raw slot id. No-op if absent.
pub fn unwatch_signal(id: u32) {
    WATCHED.with(|w| {
        w.borrow_mut().remove(&id);
    });
}

/// Drop the watch entry for a freed signal slot. Called from the arena's
/// `take_signals_batched` at the exact point the slot is freed, beside
/// the JS-notifier cleanup — eager counterpart to the lazy generation
/// pruning in the read paths.
pub(crate) fn on_signal_freed(raw: u32) {
    WATCHED.with(|w| {
        w.borrow_mut().remove(&raw);
    });
}

/// Test-only: empty the registry between cases.
#[cfg(test)]
pub(crate) fn clear() {
    WATCHED.with(|w| w.borrow_mut().clear());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reactive::{with_scope, Scope};

    #[test]
    fn watch_then_read_returns_live_debug_value() {
        clear();
        let s = Signal::new(1i32);
        watch_signal("counter", s);
        s.set(42);
        assert_eq!(read_watched_by_name("counter"), Some(serde_json::json!("42")));
        assert!(list_watched()
            .iter()
            .any(|w| w.name == "counter" && w.value == serde_json::json!("42")));
        unwatch_signal(s.id() as u32);
    }

    #[test]
    fn unwatch_removes_entry() {
        clear();
        let s = Signal::new(5i32);
        watch_signal("gone", s);
        assert!(read_watched_by_name("gone").is_some());
        unwatch_signal(s.id() as u32);
        assert!(read_watched_by_name("gone").is_none());
    }

    /// The safety-critical test: once a watched signal's scope drops and
    /// its slot is recycled by a *different* signal, the old watch entry
    /// must never serve the new occupant's value. Two layers protect
    /// this — eager pruning via `on_signal_freed` (fires during the
    /// scope's batched free) and the generation guard in the read path.
    /// (Thread-local arena: this test thread starts empty, so `fresh`
    /// reuses `stale`'s freed slot via the LIFO freelist — same premise
    /// as `reactive::stale_signal_write_after_scope_drop_is_noop_not_panic`.)
    #[test]
    fn recycled_slot_does_not_read_stale_watch_entry() {
        clear();
        let mut scope = Scope::new();
        let stale: Signal<i32> = with_scope(&mut scope, || Signal::new(7));
        let stale_gen = crate::reactive::signal_generation(stale.id());
        watch_signal("stale", stale);
        assert_eq!(read_watched_by_name("stale"), Some(serde_json::json!("7")));

        drop(scope); // frees the slot (prunes "stale") + bumps its generation

        let fresh: Signal<i32> = Signal::new(999);
        assert_eq!(fresh.id(), stale.id(), "fresh must reuse the freed slot");

        assert!(
            read_watched_by_name("stale").is_none(),
            "a disposed signal's watch entry must not survive"
        );
        assert!(
            !crate::reactive::signal_is_live(stale.id(), stale_gen),
            "the old generation must read as not-live after recycle"
        );
        // `fresh` is untouched and not exposed under the stale name.
        assert_eq!(fresh.get(), 999);
        clear();
    }

}
