//! The table-backed thread-locals Android gets (`table_thread_local!`),
//! exercised on the host: per-thread values, lazy initialisation,
//! initialisers that touch other keys, destructors at thread exit
//! (including one that touches another key), and the `LocalKey` methods.

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use runtime_tls::table_thread_local;

static INITS: AtomicUsize = AtomicUsize::new(0);

table_thread_local! {
    static COUNTER: Cell<u32> = const { Cell::new(0) };
    static NAMES: RefCell<Vec<String>> = RefCell::new(Vec::new());
    /// Counts its initialisations.
    static COUNTED: u32 = { INITS.fetch_add(1, Ordering::SeqCst); 7 };
    static DERIVED: u32 = COUNTED.with(|c| *c) * 2
}

#[test]
fn each_thread_has_its_own_value() {
    COUNTER.set(5);
    let other = std::thread::spawn(|| {
        COUNTER.set(COUNTER.get() + 1);
        COUNTER.get()
    })
    .join()
    .unwrap();
    assert_eq!(other, 1, "a new thread starts from the initialiser");
    assert_eq!(COUNTER.get(), 5);
    assert_eq!(COUNTER.replace(9), 5);
    assert_eq!(COUNTER.take(), 9);
}

#[test]
fn values_initialise_once_per_thread_and_may_read_other_keys() {
    std::thread::spawn(|| {
        let before = INITS.load(Ordering::SeqCst);
        assert_eq!(DERIVED.with(|d| *d), 14, "an initialiser reads another key");
        assert_eq!(COUNTED.with(|c| *c), 7);
        assert_eq!(INITS.load(Ordering::SeqCst), before + 1, "lazily, once");
    })
    .join()
    .unwrap();
}

#[test]
fn refcell_keys_have_the_borrow_helpers() {
    NAMES.with_borrow_mut(|v| v.push("a".into()));
    NAMES.with_borrow_mut(|v| v.push("b".into()));
    assert_eq!(NAMES.with_borrow(|v| v.join(",")), "a,b");
    assert_eq!(NAMES.replace(vec!["c".into()]), vec!["a".to_string(), "b".to_string()]);
    NAMES.set(Vec::new());
    assert!(NAMES.take().is_empty());
}

struct Dropped(Arc<AtomicUsize>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
        // A destructor may touch another thread-local, as with std.
        NAMES.with_borrow_mut(|v| v.push("dropped".into()));
    }
}

table_thread_local! {
    static HELD: RefCell<Option<Dropped>> = const { RefCell::new(None) };
}

#[test]
fn values_drop_at_thread_exit_even_when_a_drop_touches_another_key() {
    let drops = Arc::new(AtomicUsize::new(0));
    let d = drops.clone();
    std::thread::spawn(move || {
        NAMES.with_borrow_mut(|v| v.push("x".into()));
        HELD.set(Some(Dropped(d)));
    })
    .join()
    .unwrap();
    assert_eq!(drops.load(Ordering::SeqCst), 1);
}

table_thread_local! {
    static SELF_REF: u32 = SELF_REF.with(|v| *v);
}

#[test]
fn an_initialiser_reading_its_own_key_panics_and_does_not_poison_it() {
    let r = std::thread::spawn(|| SELF_REF.with(|v| *v)).join();
    assert!(r.is_err(), "recursion is reported, as std reports it");
}

table_thread_local! {
    static FLAKY: u32 = if FLAKY_ARMED.with(|a| a.replace(false)) { panic!("first try fails") } else { 3 };
    static FLAKY_ARMED: Cell<bool> = const { Cell::new(true) };
}

#[test]
fn a_panicking_initialiser_runs_again_on_the_next_access() {
    std::thread::spawn(|| {
        assert!(std::panic::catch_unwind(|| FLAKY.with(|v| *v)).is_err());
        assert_eq!(FLAKY.with(|v| *v), 3);
    })
    .join()
    .unwrap();
}

/// Off Android the crate's `thread_local!` is std's own.
#[cfg(not(target_os = "android"))]
#[test]
fn off_android_the_macro_is_std_s() {
    runtime_tls::thread_local! {
        static STD: Cell<u8> = const { Cell::new(1) };
    }
    let _: &'static std::thread::LocalKey<Cell<u8>> = &STD;
    assert_eq!(STD.get(), 1);
}
