//! `ByIdentity` / `ByIdentityArc` must satisfy the signal bound and make
//! the guarded `set` mean the right thing.
//!
//! The whole reason the type exists: `Signal<T>` is bounded on
//! `T: PartialEq` at creation and at `get`, so a payload with no value
//! equality cannot be held in app state at all — and when that payload
//! comes from a crate the author does not own, the orphan rule blocks
//! them from adding the impl themselves. Wrapping is the sanctioned
//! answer, and it is only an answer if the wrapper (a) compiles into a
//! `Signal` and (b) notifies on a genuinely new instance while staying
//! quiet on the same one.
//!
//! `by_identity.rs`'s own unit tests cover the `PartialEq`/`Hash`/`Deref`
//! algebra. These tests cover the integration that motivates it, which
//! the unit tests structurally cannot: they have no reactive kernel.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use runtime_vocabulary::glue::{self, ByIdentity, ByIdentityArc};
use runtime_world::World;

/// A stand-in for the real motivating payloads (a live `MediaStream`, a
/// third-party session object, an `Arc<dyn Storage>`): interior state, a
/// live resource, and deliberately **no `PartialEq`**. Adding one here
/// would defeat the test.
struct Session {
    label: &'static str,
    pings: Cell<u32>,
}

impl Session {
    fn open(label: &'static str) -> Self {
        Session { label, pings: Cell::new(0) }
    }
    fn ping(&self) {
        self.pings.set(self.pings.get() + 1);
    }
}

/// Count notifications from an effect subscribed to `sig`.
fn watch_count<T: PartialEq + Clone + 'static>(sig: glue::Signal<T>) -> Rc<Cell<usize>> {
    let n = Rc::new(Cell::new(0));
    let sink = n.clone();
    let _ = glue::effect(move || {
        sig.with(|_| ());
        sink.set(sink.get() + 1);
    });
    n
}

/// The load-bearing test. Same instance stored again → the equality guard
/// swallows the write and no subscriber runs. A different instance → the
/// guard sees a different pointer and the subscriber runs.
#[test]
fn by_identity_in_a_signal_guards_on_instance_not_value() {
    let world = World::new();
    let (sig, runs) = world.enter(|| {
        let sig = glue::signal(ByIdentity::new(Session::open("a")));
        let runs = watch_count(sig);
        (sig, runs)
    });
    world.flush();
    assert_eq!(runs.get(), 1, "the effect runs once on creation");

    // Storing the SAME instance (a clone of what is already in the
    // signal) must not notify — this is what `Rc::ptr_eq` buys.
    let same = world.enter(|| sig.get());
    world.enter(|| sig.set(same));
    world.flush();
    assert_eq!(
        runs.get(),
        1,
        "re-storing the same instance must be swallowed by the guarded set"
    );

    // A DIFFERENT instance must notify, even though nothing about the
    // payload's observable state changed — identity is the question.
    world.enter(|| sig.set(ByIdentity::new(Session::open("a"))));
    world.flush();
    assert_eq!(
        runs.get(),
        2,
        "a freshly allocated instance is a different instance and must notify"
    );
}

/// A wrapper built from a clone of the *caller's* `Rc` is the same
/// instance, so `from_ptr` round-trips through a signal without a
/// spurious notify. This is the shape an author hits when they keep the
/// `Rc` around and re-wrap it.
#[test]
fn by_identity_from_ptr_is_the_same_instance_through_a_signal() {
    let world = World::new();
    let rc = Rc::new(Session::open("shared"));

    let (sig, runs) = {
        let rc = rc.clone();
        world.enter(move || {
            let sig = glue::signal(ByIdentity::from_ptr(rc));
            let runs = watch_count(sig);
            (sig, runs)
        })
    };
    world.flush();
    assert_eq!(runs.get(), 1);

    world.enter(|| sig.set(ByIdentity::from_ptr(rc.clone())));
    world.flush();
    assert_eq!(
        runs.get(),
        1,
        "re-wrapping the same Rc must not read as a new instance"
    );
}

/// The payload stays usable through the wrapper — `Deref` reaches its
/// methods with no unwrapping, and interior state survives the round
/// trip through the signal.
#[test]
fn by_identity_payload_is_reachable_through_the_signal() {
    let world = World::new();
    let sig = world.enter(|| glue::signal(ByIdentity::new(Session::open("live"))));

    world.enter(|| {
        sig.with(|s| {
            s.ping();
            s.ping();
        })
    });

    world.enter(|| {
        sig.with(|s| {
            assert_eq!(s.label, "live");
            assert_eq!(s.pings.get(), 2, "interior state survives; the wrapper shares, not copies");
        })
    });
}

/// `Option<ByIdentity<_>>` is the shape every hand-rolled workaround in
/// the tree actually used (a stream slot that starts empty). `None` →
/// `Some` must notify, and `Some(x)` → `Some(x)` must not.
#[test]
fn optional_by_identity_notifies_on_fill_but_not_on_restore() {
    let world = World::new();
    let (sig, runs) = world.enter(|| {
        let sig = glue::signal::<Option<ByIdentity<Session>>>(None);
        let runs = watch_count(sig);
        (sig, runs)
    });
    world.flush();
    assert_eq!(runs.get(), 1);

    world.enter(|| sig.set(Some(ByIdentity::new(Session::open("cam")))));
    world.flush();
    assert_eq!(runs.get(), 2, "None → Some must notify");

    let same = world.enter(|| sig.get());
    world.enter(|| sig.set(same));
    world.flush();
    assert_eq!(runs.get(), 2, "Some(x) → Some(same x) must be guarded");

    world.enter(|| sig.set(None));
    world.flush();
    assert_eq!(runs.get(), 3, "Some → None must notify");
}

/// The `Arc` sibling exists for values handed to the author as
/// `Arc<dyn Trait>` (`storage::platform_storage()` is the shipped case).
/// Wrapping clones of ONE `Arc` must stay quiet; a second `Arc` over an
/// equal value must notify — value equality is explicitly NOT the test.
#[test]
fn by_identity_arc_in_a_signal_guards_on_instance_not_value() {
    trait Store {
        fn name(&self) -> &str;
    }
    struct Mem(String);
    impl Store for Mem {
        fn name(&self) -> &str {
            &self.0
        }
    }

    let world = World::new();
    let first: Arc<dyn Store> = Arc::new(Mem("local".into()));

    let (sig, runs) = {
        let first = first.clone();
        world.enter(move || {
            let sig = glue::signal(ByIdentityArc::from_ptr(first));
            let runs = watch_count(sig);
            (sig, runs)
        })
    };
    world.flush();
    assert_eq!(runs.get(), 1);

    world.enter(|| sig.set(ByIdentityArc::from_ptr(first.clone())));
    world.flush();
    assert_eq!(runs.get(), 1, "a clone of the same Arc is the same instance");

    // Same NAME, different allocation → notify.
    let second: Arc<dyn Store> = Arc::new(Mem("local".into()));
    world.enter(|| sig.set(ByIdentityArc::from_ptr(second)));
    world.flush();
    assert_eq!(
        runs.get(),
        2,
        "an equal-valued but independently allocated Arc is a different instance"
    );

    world.enter(|| sig.with(|s| assert_eq!(s.name(), "local")));
}

/// Regression pin on the reason the type is in `runtime-shared` and
/// re-exported through `glue`: an app crate spells it
/// `runtime_core::ByIdentity`, and `runtime_core::*` IS
/// `runtime_vocabulary::glue::*`. If the re-export is dropped the path
/// breaks for every author even though the type still exists.
#[test]
fn by_identity_is_reachable_from_the_author_surface() {
    let _: fn(Session) -> ByIdentity<Session> = glue::ByIdentity::<Session>::new;
    let _: fn(String) -> ByIdentityArc<String> = glue::ByIdentityArc::<String>::new;
    // …and it is the SAME type as the one runtime-shared defines, not a
    // parallel copy.
    let a: runtime_shared::ByIdentity<u8> = glue::ByIdentity::new(1u8);
    let b: glue::ByIdentity<u8> = a.clone();
    assert_eq!(a, b);
}

/// Guard against the tempting-but-wrong alternative: a wrapper that
/// compares by VALUE would have made the same-instance set a no-op only
/// by accident, and would have made a second, equal-valued instance
/// invisible. Documented here because the failure is silent — the app
/// just stops re-rendering when a stream is swapped for a fresh one.
#[test]
fn by_identity_distinguishes_two_structurally_identical_payloads() {
    let seen: Rc<RefCell<Vec<u32>>> = Rc::new(RefCell::new(Vec::new()));
    let world = World::new();

    let sig = world.enter(|| {
        let sig = glue::signal(ByIdentity::new(Session::open("same-label")));
        let sink = seen.clone();
        let _ = glue::effect(move || {
            sig.with(|s| sink.borrow_mut().push(s.pings.get()));
        });
        sig
    });
    world.flush();

    // Three separately opened sessions, all structurally identical.
    for _ in 0..3 {
        world.enter(|| sig.set(ByIdentity::new(Session::open("same-label"))));
        world.flush();
    }

    assert_eq!(
        seen.borrow().len(),
        4,
        "each new instance must notify: creation + 3 swaps"
    );
}
