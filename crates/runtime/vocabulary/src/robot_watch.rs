//! Live signal-watch registry for the NEW core — the vocabulary port of
//! `runtime_shared::robot::watch` (P5 `watch_signal` seam). Whole module
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

use runtime_shared::__serde_json as serde_json;
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
    /// TRACKED `Debug` render — read inside the history effect, so the
    /// effect re-runs on every change (module docs, "History").
    fn watch_read_tracked(&self) -> String;
}

impl<T: PartialEq + Clone + Debug + 'static> WatchTarget<T> for Signal<T> {
    fn watch_raw_id(&self) -> u64 {
        self.raw_id()
    }
    fn watch_read(&self) -> String {
        untrack(|| self.with(|v| format!("{v:?}")))
    }
    fn watch_read_tracked(&self) -> String {
        self.with(|v| format!("{v:?}"))
    }
}

impl<T: PartialEq + Clone + Debug + 'static> WatchTarget<T> for ReadSignal<T> {
    fn watch_raw_id(&self) -> u64 {
        self.raw_id()
    }
    fn watch_read(&self) -> String {
        untrack(|| self.with(|v| format!("{v:?}")))
    }
    fn watch_read_tracked(&self) -> String {
        self.with(|v| format!("{v:?}"))
    }
}

impl<T: PartialEq + Clone + Debug + 'static> WatchTarget<T> for Memo<T> {
    fn watch_raw_id(&self) -> u64 {
        self.raw_id()
    }
    fn watch_read(&self) -> String {
        untrack(|| format!("{:?}", self.get()))
    }
    fn watch_read_tracked(&self) -> String {
        format!("{:?}", self.get())
    }
}

/// How many past values a watched signal keeps (oldest dropped first).
pub const WATCH_HISTORY_LEN: usize = 64;

/// A watched signal's change log, kept by its history effect.
#[derive(Default)]
struct History {
    /// Changes since the watch began (the initial value is not one).
    writes: u64,
    /// `(time_source micros, Debug render)`, oldest first. The first
    /// entry is the value at watch time. `0` micros = no clock installed.
    values: std::collections::VecDeque<(u64, String)>,
}

/// Parses a JSON value into the signal's type and sets it (the
/// `write_signal` verb). Only [`watch_signal_writable`] provides one.
type Writer = Rc<dyn Fn(&Value) -> Result<(), String>>;

struct WatchEntry {
    name: String,
    /// Full identity of the registering handle — teardown removes the
    /// entry only while it still owns the slot (module docs).
    raw_id: u64,
    /// Untracked read → JSON. Only invoked while the entry is alive,
    /// which the scope-tied teardown guarantees means the slot is live.
    reader: Rc<dyn Fn() -> Value>,
    history: Rc<RefCell<History>>,
    writer: Option<Writer>,
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
    S: WatchTarget<T> + Copy + 'static,
{
    register_watch(name.into(), signal, None);
}

/// [`watch_signal`] for a unified [`Signal`] whose value the bridge may
/// also SET (`write_signal`: the new value arrives as JSON and is parsed
/// into `T`). The inspector shows its "Set value" field only for these.
pub fn watch_signal_writable<T>(name: impl Into<String>, signal: Signal<T>)
where
    T: PartialEq + Clone + Debug + serde::de::DeserializeOwned + 'static,
{
    let writer: Writer = Rc::new(move |value: &Value| {
        let parsed: T = serde_json::from_value(value.clone())
            .map_err(|e| format!("value does not parse as the signal's type: {e}"))?;
        signal.set(parsed);
        Ok(())
    });
    register_watch(name.into(), signal, Some(writer));
}

fn register_watch<T, S>(name: String, signal: S, writer: Option<Writer>)
where
    T: PartialEq + Clone + Debug + 'static,
    S: WatchTarget<T> + Copy + 'static,
{
    let raw_id = signal.watch_raw_id();
    let slot = (raw_id & 0xffff_ffff) as u32;
    let reader: Rc<dyn Fn() -> Value> =
        Rc::new(move || Value::String(signal.watch_read()));
    let history: Rc<RefCell<History>> = Rc::default();
    WATCHED.with(|w| {
        w.borrow_mut().insert(
            slot,
            WatchEntry { name, raw_id, reader, history: history.clone(), writer },
        );
    });
    // History: an effect that reads the signal TRACKED, so it re-runs on
    // every committed change and logs it. It lives in the same ambient
    // scope as the teardown probe below, so it dies with the entry — and
    // a scope drop frees effects before signals, so it never reads a
    // freed slot. It writes only this module's thread-local, never a
    // signal, so it cannot feed back into the app.
    let _ = runtime_world::effect(move || {
        let value = signal.watch_read_tracked();
        let mut h = history.borrow_mut();
        if !h.values.is_empty() {
            h.writes += 1;
        }
        if h.values.len() == WATCH_HISTORY_LEN {
            h.values.pop_front();
        }
        h.values.push_back((runtime_shared::time::now_micros(), value));
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

/// Milliseconds since `at` (a `now_micros` reading), or `None` when no
/// clock was installed when either reading was taken.
fn ago_ms(at: u64) -> Option<u64> {
    let now = runtime_shared::time::now_micros();
    (at != 0 && now != 0).then(|| now.saturating_sub(at) / 1000)
}

/// One watched signal's current state (`list_watched_signals` verb).
pub struct WatchedSnapshot {
    pub id: u32,
    pub name: String,
    pub value: Value,
    /// Changes since the watch began.
    pub writes: u64,
    /// Milliseconds since the last change (or since the watch began, when
    /// it never changed); `None` without a clock.
    pub changed_ago_ms: Option<u64>,
    /// Registered with [`watch_signal_writable`].
    pub writable: bool,
}

/// Snapshot every watched signal with its current value, name-sorted
/// for a stable inspector display. Readers run after the registry
/// borrow drops (a reader could defensively re-enter this module) and
/// world-ENTERED via the installed driver env (self-wrapping, like the
/// Robot's label queries — the harness/bridge need not wrap).
pub fn list_watched() -> Vec<WatchedSnapshot> {
    type Row = (u32, String, Rc<dyn Fn() -> Value>, Rc<RefCell<History>>, bool);
    let entries: Vec<Row> = WATCHED.with(|w| {
        w.borrow()
            .iter()
            .map(|(id, e)| (*id, e.name.clone(), e.reader.clone(), e.history.clone(), e.writer.is_some()))
            .collect()
    });
    let mut out: Vec<WatchedSnapshot> = crate::robot::entered(|| {
        entries
            .into_iter()
            .map(|(id, name, reader, history, writable)| {
                let h = history.borrow();
                WatchedSnapshot {
                    id,
                    name,
                    value: reader(),
                    writes: h.writes,
                    changed_ago_ms: h.values.back().and_then(|(at, _)| ago_ms(*at)),
                    writable,
                }
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

/// One recorded value of a watched signal.
pub struct HistoryPoint {
    /// Milliseconds before now; `None` without a clock.
    pub ago_ms: Option<u64>,
    pub value: String,
}

/// A watched signal's recorded values, oldest first (at most
/// [`WATCH_HISTORY_LEN`]; the first is the value when watching began).
pub fn watched_history(id: u32) -> Option<(String, u64, Vec<HistoryPoint>)> {
    WATCHED.with(|w| {
        let w = w.borrow();
        let e = w.get(&id)?;
        let h = e.history.borrow();
        let points = h
            .values
            .iter()
            .map(|(at, value)| HistoryPoint { ago_ms: ago_ms(*at), value: value.clone() })
            .collect();
        Some((e.name.clone(), h.writes, points))
    })
}

/// Resolve a wire `id` or `name` to a slot id.
pub fn resolve_watched(id: Option<u32>, name: Option<&str>) -> Option<u32> {
    WATCHED.with(|w| {
        let w = w.borrow();
        match (id, name) {
            (Some(id), _) => w.contains_key(&id).then_some(id),
            (None, Some(name)) => w.iter().find(|(_, e)| e.name == name).map(|(id, _)| *id),
            (None, None) => None,
        }
    })
}

/// Set a writable watched signal from JSON (`write_signal`). The write is
/// staged and then settled, so a read on the next line sees it — the
/// same action contract as `invoke_method`.
pub fn write_watched(id: u32, value: &Value) -> Result<(), String> {
    let writer = WATCHED.with(|w| {
        let w = w.borrow();
        let e = w.get(&id).ok_or_else(|| format!("no watched signal with id {id}"))?;
        e.writer.clone().ok_or_else(|| {
            format!(
                "signal '{}' is read-only over the bridge; register it with \
                 `robot::watch_signal_writable` to allow writes",
                e.name
            )
        })
    })?;
    writer(value)?;
    crate::robot::settle();
    Ok(())
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

    /// Each committed change lands in the history, and only changes count
    /// as writes (the value at watch time is the baseline, not a write).
    #[test]
    fn history_records_committed_changes() {
        reset();
        let world = World::new();
        let s = world.enter(|| {
            let s = signal(1i32);
            watch_signal("n", s);
            s
        });
        world.enter(|| s.set(2));
        world.flush();
        world.enter(|| s.set(3));
        world.flush();
        let id = resolve_watched(None, Some("n")).expect("watched");
        let (name, writes, points) = watched_history(id).expect("history");
        assert_eq!(name, "n");
        assert_eq!(writes, 2);
        let values: Vec<&str> = points.iter().map(|p| p.value.as_str()).collect();
        assert_eq!(values, ["1", "2", "3"]);
        let row = world.enter(list_watched).into_iter().find(|w| w.name == "n").unwrap();
        assert_eq!((row.writes, row.writable), (2, false));
        reset();
    }

    #[test]
    fn history_keeps_the_newest_values() {
        reset();
        let world = World::new();
        let s = world.enter(|| {
            let s = signal(0usize);
            watch_signal("n", s);
            s
        });
        for i in 1..=(WATCH_HISTORY_LEN + 5) {
            world.enter(|| s.set(i));
            world.flush();
        }
        let id = resolve_watched(None, Some("n")).unwrap();
        let (_, writes, points) = watched_history(id).unwrap();
        assert_eq!(writes as usize, WATCH_HISTORY_LEN + 5);
        assert_eq!(points.len(), WATCH_HISTORY_LEN);
        assert_eq!(points.last().unwrap().value, (WATCH_HISTORY_LEN + 5).to_string());
        reset();
    }

    #[test]
    fn writable_watch_parses_and_sets_read_only_refuses() {
        reset();
        let world = World::new();
        let (rw, ro) = world.enter(|| {
            let rw = signal(1i32);
            let ro = signal(String::from("x"));
            watch_signal_writable("rw", rw);
            watch_signal("ro", ro);
            (rw, ro)
        });
        let rw_id = resolve_watched(None, Some("rw")).unwrap();
        let ro_id = resolve_watched(None, Some("ro")).unwrap();
        world.enter(|| write_watched(rw_id, &serde_json::json!(9))).expect("parses as i32");
        world.flush();
        assert_eq!(world.enter(|| rw.get()), 9);
        let err = world.enter(|| write_watched(rw_id, &serde_json::json!("nine"))).unwrap_err();
        assert!(err.contains("does not parse"), "{err}");
        let err = world.enter(|| write_watched(ro_id, &serde_json::json!("y"))).unwrap_err();
        assert!(err.contains("read-only"), "{err}");
        assert_eq!(world.enter(|| ro.get()), "x");
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
